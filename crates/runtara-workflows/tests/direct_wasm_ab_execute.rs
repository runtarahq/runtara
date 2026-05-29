//! Direct-vs-components execution parity smoke test.
//!
//! Gated by `RUNTARA_RUN_DIRECT_WASM_E2E=1` because it needs cargo-component,
//! prebuilt shared workflow components, `wac`, and `wasmtime`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use base64::Engine;
use runtara_workflows::direct_wasm::DIRECT_SHARED_COMPONENT_REQUIREMENTS;
use runtara_workflows::{
    CompilationInput, DirectWorkflowCompileOptions, ExecutionGraph, WorkflowCompilerMode,
    compile_workflow, compile_workflow_direct,
};
use serde_json::Value;
use tempfile::TempDir;

const SIMPLE_PASSTHROUGH: &str = include_str!("fixtures/simple_passthrough.json");
const CONDITIONAL_WORKFLOW: &str = include_str!("fixtures/conditional_workflow.json");
const FILTER_SIMPLE: &str = include_str!("fixtures/filter_simple.json");
const SWITCH_VALUE_SIMPLE: &str = include_str!("fixtures/switch_value_simple.json");
const GROUP_BY_SIMPLE: &str = include_str!("fixtures/group_by_simple.json");
const EDGE_CONDITION_PRIORITY: &str = include_str!("fixtures/edge_condition_priority.json");
const WHILE_DIRECT_INDEX_ONLY: &str = include_str!("fixtures/while_direct_index_only.json");
const LOG_ALL_LEVELS: &str = include_str!("fixtures/log_all_levels.json");
const ERROR_DIRECT_SIMPLE: &str = include_str!("fixtures/error_direct_simple.json");
const DELAY_DYNAMIC: &str = include_str!("fixtures/delay_dynamic.json");
const AGENT_CACHE_KEY: &str = "agent::utils::return-input::agent";
const SPLIT_CACHE_KEY: &str = "split::split";
const SPLIT_FINISH_WITH_SCHEMAS: &str = r#"{
  "durable": true,
  "steps": {
    "split": {
      "stepType": "Split",
      "id": "split",
      "config": {
        "value": { "valueType": "reference", "value": "data.items" },
        "sequential": true
      },
      "inputSchema": {
        "value": { "type": "string", "required": true }
      },
      "outputSchema": {
        "value": { "type": "string", "required": true }
      },
      "subgraph": {
        "name": "Echo Item",
        "steps": {
          "finish": {
            "stepType": "Finish",
            "id": "finish",
            "inputMapping": {
              "value": { "valueType": "reference", "value": "data.value" },
              "index": { "valueType": "reference", "value": "variables._index" },
              "indices": { "valueType": "reference", "value": "variables._loop_indices" }
            }
          }
        },
        "entryPoint": "finish",
        "executionPlan": []
      }
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "results": { "valueType": "reference", "value": "steps.split.outputs" }
      }
    }
  },
  "entryPoint": "split",
  "executionPlan": [
    { "fromStep": "split", "toStep": "finish" }
  ],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}"#;
const AGENT_RETURN_INPUT: &str = r#"{
  "durable": true,
  "steps": {
    "agent": {
      "stepType": "Agent",
      "id": "agent",
      "name": "Return Input",
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

#[derive(Debug, PartialEq, Eq)]
struct SleepRequest {
    checkpoint_id: String,
    duration_ms: u64,
    state: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
struct CheckpointRequest {
    checkpoint_id: String,
    state: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
struct SignalAckRequest {
    signal_type: String,
}

#[derive(Debug)]
enum CapturedMessage {
    Completed(Completed),
    Failed(Failed),
    Event(RuntimeEvent),
    Sleep(SleepRequest),
    Checkpoint(CheckpointRequest),
    Suspended,
    SignalAck(SignalAckRequest),
}

#[derive(Debug)]
struct CapturedRun {
    output_json: Option<Value>,
    error_json: Option<Value>,
    events: Vec<RuntimeEvent>,
    sleeps: Vec<SleepRequest>,
    checkpoints: Vec<CheckpointRequest>,
    suspended_count: usize,
    signal_acks: Vec<SignalAckRequest>,
    status_success: bool,
    stderr: String,
}

struct ServerState {
    checkpoints: Mutex<HashMap<String, Vec<u8>>>,
    pending_checkpoint_signal: Mutex<Option<String>>,
}

impl ServerState {
    fn new(
        preloaded_checkpoints: Vec<(String, Vec<u8>)>,
        pending_checkpoint_signal: Option<String>,
    ) -> Self {
        Self {
            checkpoints: Mutex::new(preloaded_checkpoints.into_iter().collect()),
            pending_checkpoint_signal: Mutex::new(pending_checkpoint_signal),
        }
    }
}

struct DirectArtifact {
    path: PathBuf,
    compiler_mode: WorkflowCompilerMode,
    _temp: TempDir,
}

struct AbCase {
    name: &'static str,
    graph_json: &'static str,
    inputs: &'static [&'static [u8]],
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

fn cargo_component_installed() -> bool {
    Command::new("cargo")
        .arg("component")
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
        if !stdlib_bytes
            .windows(b"split-cache-key".len())
            .any(|window| window == b"split-cache-key")
        {
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

fn direct_ab_components_dir() -> Option<PathBuf> {
    if !e2e_enabled() {
        eprintln!(
            "SKIP: direct_wasm_ab_execute - set RUNTARA_RUN_DIRECT_WASM_E2E=1 to run \
             (needs cargo-component, wac, wasmtime, and staged direct workflow components)."
        );
        return None;
    }
    if !cargo_component_installed() {
        eprintln!("SKIP: cargo-component not installed.");
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
    shared_components_dir()
}

fn setup_data_dir() -> Option<TempDir> {
    if std::env::var_os("DATA_DIR").is_some() {
        return None;
    }
    let temp = TempDir::new().expect("tempdir");
    // SAFETY: this gated integration test is run with --test-threads=1 in CI,
    // and env mutation happens before any workflow execution starts.
    unsafe {
        std::env::set_var("DATA_DIR", temp.path());
        if std::env::var_os("RUNTARA_COMPONENTS_TARGET_DIR").is_none() {
            std::env::set_var(
                "RUNTARA_COMPONENTS_TARGET_DIR",
                temp.path().join("shared-target"),
            );
        }
    }
    Some(temp)
}

fn graph_from_fixture(graph_json: &str) -> ExecutionGraph {
    serde_json::from_str(graph_json).expect("fixture parses")
}

fn compile_components_artifact(workflow_id: &str, graph_json: &str) -> PathBuf {
    let compiled = compile_workflow(CompilationInput {
        tenant_id: "direct-wasm-ab".to_string(),
        workflow_id: format!("ab-components-{workflow_id}"),
        version: 1,
        execution_graph: graph_from_fixture(graph_json),
        track_events: false,
        child_workflows: vec![],
        connection_service_url: None,
        agent_catalog: None,
        progress_callback: None,
    })
    .expect("components compile succeeds");

    assert_eq!(
        compiled.compiler_mode,
        WorkflowCompilerMode::ComponentsCodegen
    );
    assert!(compiled.binary_path.exists(), "components wasm missing");
    compiled.binary_path
}

fn compile_direct_artifact(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
) -> DirectArtifact {
    let temp = TempDir::new().expect("tempdir");
    let compiled = compile_workflow_direct(
        CompilationInput {
            tenant_id: "direct-wasm-ab".to_string(),
            workflow_id: format!("ab-direct-{workflow_id}"),
            version: 1,
            execution_graph: graph_from_fixture(graph_json),
            track_events: false,
            child_workflows: vec![],
            connection_service_url: None,
            agent_catalog: None,
            progress_callback: None,
        },
        DirectWorkflowCompileOptions {
            output_dir: temp.path().to_path_buf(),
            components_dir: components_dir.to_path_buf(),
            source_checksum: None,
        },
    )
    .expect("direct compile succeeds");

    assert_eq!(compiled.compiler_mode, WorkflowCompilerMode::DirectWasm);
    assert!(compiled.binary_path.exists(), "direct wasm missing");
    DirectArtifact {
        path: compiled.binary_path,
        compiler_mode: compiled.compiler_mode,
        _temp: temp,
    }
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
            ("POST", "failed") => {
                capture_failed(body, sink);
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
            ("POST", "suspended") => {
                let _ = sink.send(CapturedMessage::Suspended);
                return (200, serde_json::json!({"success": true}));
            }
            ("POST", "signals/ack") => {
                capture_signal_ack(body, sink);
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
    if let Some(existing) = checkpoints
        .get(&checkpoint_id)
        .or_else(|| checkpoints.get(&normalized_checkpoint_id(&checkpoint_id)))
    {
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

    let mut pending_signal = None;
    if !state.is_empty() {
        checkpoints.insert(checkpoint_id.clone(), state.clone());
        checkpoints.insert(normalized_checkpoint_id(&checkpoint_id), state);
        pending_signal = server_state
            .pending_checkpoint_signal
            .lock()
            .expect("checkpoint signal lock")
            .take()
            .map(|signal_type| {
                serde_json::json!({
                    "signal_type": signal_type,
                    "payload": null,
                })
            });
    }

    (
        200,
        serde_json::json!({
            "found": false,
            "state": null,
            "signal": pending_signal,
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

fn capture_signal_ack(body: &[u8], sink: &mpsc::Sender<CapturedMessage>) {
    if let Ok(parsed) = serde_json::from_slice::<Value>(body)
        && let Some(signal_type) = parsed.get("signal_type").and_then(Value::as_str)
    {
        let _ = sink.send(CapturedMessage::SignalAck(SignalAckRequest {
            signal_type: signal_type.to_string(),
        }));
    }
}

fn serve(
    listener: TcpListener,
    sink: mpsc::Sender<CapturedMessage>,
    stop: mpsc::Receiver<()>,
    server_state: Arc<ServerState>,
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

fn execute_artifact(binary_path: &Path, instance_id: &str, workflow_input: &[u8]) -> CapturedRun {
    execute_artifact_with_preloaded_checkpoints(
        binary_path,
        instance_id,
        workflow_input,
        Vec::new(),
    )
}

fn execute_artifact_with_preloaded_checkpoints(
    binary_path: &Path,
    instance_id: &str,
    workflow_input: &[u8],
    preloaded_checkpoints: Vec<(String, Vec<u8>)>,
) -> CapturedRun {
    execute_artifact_with_state(
        binary_path,
        instance_id,
        workflow_input,
        preloaded_checkpoints,
        None,
    )
}

fn execute_artifact_with_checkpoint_signal(
    binary_path: &Path,
    instance_id: &str,
    workflow_input: &[u8],
    signal_type: &str,
) -> CapturedRun {
    execute_artifact_with_state(
        binary_path,
        instance_id,
        workflow_input,
        Vec::new(),
        Some(signal_type.to_string()),
    )
}

fn execute_artifact_with_state(
    binary_path: &Path,
    instance_id: &str,
    workflow_input: &[u8],
    preloaded_checkpoints: Vec<(String, Vec<u8>)>,
    pending_checkpoint_signal: Option<String>,
) -> CapturedRun {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let (capture_tx, capture_rx) = mpsc::channel::<CapturedMessage>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let server_state = Arc::new(ServerState::new(
        preloaded_checkpoints,
        pending_checkpoint_signal,
    ));
    let workflow_input = Arc::new(workflow_input.to_vec());
    let server_handle =
        thread::spawn(move || serve(listener, capture_tx, stop_rx, server_state, workflow_input));

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
        .arg(format!("RUNTARA_INSTANCE_ID={instance_id}"))
        .arg("--env")
        .arg("RUNTARA_TENANT_ID=direct-wasm-ab")
        .arg("--env")
        .arg("RUST_LOG=warn")
        .arg(binary_path)
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
    let mut suspended_count = 0usize;
    let mut signal_acks = Vec::new();
    for message in capture_rx.try_iter() {
        match message {
            CapturedMessage::Completed(completed) => output_json = Some(completed.output_json),
            CapturedMessage::Failed(failed) => error_json = Some(failed.error_json),
            CapturedMessage::Event(event) => events.push(event),
            CapturedMessage::Sleep(sleep) => sleeps.push(sleep),
            CapturedMessage::Checkpoint(checkpoint) => checkpoints.push(checkpoint),
            CapturedMessage::Suspended => suspended_count += 1,
            CapturedMessage::SignalAck(signal_ack) => signal_acks.push(signal_ack),
        }
    }

    CapturedRun {
        output_json,
        error_json,
        events,
        sleeps,
        checkpoints,
        suspended_count,
        signal_acks,
        status_success: output.status.success(),
        stderr: stderr.into_owned(),
    }
}

fn components_sdk_input(workflow_input: &[u8]) -> Vec<u8> {
    let data: Value = serde_json::from_slice(workflow_input).expect("workflow input parses");
    serde_json::to_vec(&serde_json::json!({
        "data": data,
        "variables": {},
    }))
    .expect("components sdk input serializes")
}

fn normalized_event_payload(mut payload: Value) -> Value {
    if let Some(object) = payload.as_object_mut() {
        object.remove("timestamp_ms");
    }
    payload
}

fn normalized_events(events: &[RuntimeEvent]) -> Vec<(String, Value)> {
    events
        .iter()
        .map(|event| {
            (
                event.subtype.clone(),
                normalized_event_payload(event.payload_json.clone()),
            )
        })
        .collect()
}

fn normalized_checkpoint_id(checkpoint_id: &str) -> String {
    // Rust-generated components wrap checkpoint ids with the resilient
    // function and workflow instance prefix; direct artifacts own only the
    // stable step key suffix today. If the step id itself is "agent", direct
    // keys can surface as "<step>::agent::<agent-id>::<capability>::<step>";
    // compare from the generated-Rust cache key base onward.
    let suffix = checkpoint_id
        .find("agent::")
        .or_else(|| checkpoint_id.find("split::"))
        .map(|index| checkpoint_id[index..].to_string())
        .unwrap_or_else(|| checkpoint_id.to_string());
    let segments = suffix.split("::").collect::<Vec<_>>();
    if segments.len() >= 5 && segments[1] == "agent" && segments[0] == segments[4] {
        return segments[1..].join("::");
    }
    suffix
}

fn normalized_checkpoints(checkpoints: &[CheckpointRequest]) -> Vec<(String, Vec<u8>)> {
    checkpoints
        .iter()
        .map(|checkpoint| {
            (
                normalized_checkpoint_id(&checkpoint.checkpoint_id),
                checkpoint.state.clone(),
            )
        })
        .collect()
}

fn assert_success_parity(
    case_name: &str,
    input_index: usize,
    components: &CapturedRun,
    direct: &CapturedRun,
) {
    assert!(
        components.status_success,
        "components artifact failed for {case_name}[{input_index}]:\n{}",
        components.stderr
    );
    assert!(
        direct.status_success,
        "direct artifact failed for {case_name}[{input_index}]:\n{}",
        direct.stderr
    );
    assert!(
        components.error_json.is_none(),
        "components artifact unexpectedly failed for {case_name}[{input_index}]: {:?}",
        components.error_json
    );
    assert!(
        direct.error_json.is_none(),
        "direct artifact unexpectedly failed for {case_name}[{input_index}]: {:?}",
        direct.error_json
    );
    assert_eq!(
        components.output_json, direct.output_json,
        "completion payload mismatch for {case_name}[{input_index}]"
    );
    assert!(
        components.output_json.is_some(),
        "components artifact did not POST /completed for {case_name}[{input_index}]"
    );
    assert_eq!(
        normalized_events(&components.events),
        normalized_events(&direct.events),
        "custom-event payload mismatch for {case_name}[{input_index}]"
    );
    assert_eq!(
        components.sleeps, direct.sleeps,
        "durable sleep request mismatch for {case_name}[{input_index}]"
    );
    assert_eq!(
        normalized_checkpoints(&components.checkpoints),
        normalized_checkpoints(&direct.checkpoints),
        "checkpoint request mismatch for {case_name}[{input_index}]"
    );
    assert_eq!(
        components.suspended_count, direct.suspended_count,
        "suspended lifecycle request mismatch for {case_name}[{input_index}]"
    );
    assert_eq!(
        components.signal_acks, direct.signal_acks,
        "signal acknowledgement mismatch for {case_name}[{input_index}]"
    );
}

fn assert_failure_parity(
    case_name: &str,
    input_index: usize,
    components: &CapturedRun,
    direct: &CapturedRun,
) {
    assert!(
        !components.status_success,
        "components artifact should have failed for {case_name}[{input_index}]"
    );
    assert!(
        !direct.status_success,
        "direct artifact should have failed for {case_name}[{input_index}]"
    );
    assert!(
        components.output_json.is_none(),
        "components artifact unexpectedly completed for {case_name}[{input_index}]: {:?}",
        components.output_json
    );
    assert!(
        direct.output_json.is_none(),
        "direct artifact unexpectedly completed for {case_name}[{input_index}]: {:?}",
        direct.output_json
    );
    assert_eq!(
        components.error_json, direct.error_json,
        "failure payload mismatch for {case_name}[{input_index}]"
    );
    assert!(
        components.error_json.is_some(),
        "components artifact did not POST /failed for {case_name}[{input_index}]"
    );
    assert_eq!(
        normalized_events(&components.events),
        normalized_events(&direct.events),
        "failure custom-event payload mismatch for {case_name}[{input_index}]"
    );
    assert_eq!(
        components.sleeps, direct.sleeps,
        "failure durable sleep request mismatch for {case_name}[{input_index}]"
    );
    assert_eq!(
        normalized_checkpoints(&components.checkpoints),
        normalized_checkpoints(&direct.checkpoints),
        "failure checkpoint request mismatch for {case_name}[{input_index}]"
    );
    assert_eq!(
        components.suspended_count, direct.suspended_count,
        "failure suspended lifecycle request mismatch for {case_name}[{input_index}]"
    );
    assert_eq!(
        components.signal_acks, direct.signal_acks,
        "failure signal acknowledgement mismatch for {case_name}[{input_index}]"
    );
}

#[test]
fn direct_wasm_matches_components_execution_for_supported_json_fixtures() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    const CASES: &[AbCase] = &[
        AbCase {
            name: "finish-passthrough",
            graph_json: SIMPLE_PASSTHROUGH,
            inputs: &[br#"{"input":"ab-finish"}"#],
        },
        AbCase {
            name: "conditional",
            graph_json: CONDITIONAL_WORKFLOW,
            inputs: &[br#"{"flag":true}"#, br#"{"flag":false}"#],
        },
        AbCase {
            name: "filter",
            graph_json: FILTER_SIMPLE,
            inputs: &[br#"{"items":[{"id":1,"status":"active"},{"id":2,"status":"failed"},{"id":3,"status":"active"}]}"#],
        },
        AbCase {
            name: "value-switch",
            graph_json: SWITCH_VALUE_SIMPLE,
            inputs: &[br#"{"status":"active"}"#, br#"{"status":"retry"}"#],
        },
        AbCase {
            name: "group-by",
            graph_json: GROUP_BY_SIMPLE,
            inputs: &[br#"{"items":[{"id":1,"status":"active"},{"id":2,"status":"inactive"},{"id":3,"status":"active"}]}"#],
        },
        AbCase {
            name: "edge-condition",
            graph_json: EDGE_CONDITION_PRIORITY,
            inputs: &[
                br#"{"status":"active","tier":"vip"}"#,
                br#"{"status":"active","tier":"basic"}"#,
                br#"{"status":"inactive","tier":"basic"}"#,
            ],
        },
        AbCase {
            name: "split-schema",
            graph_json: SPLIT_FINISH_WITH_SCHEMAS,
            inputs: &[br#"{"items":[{"value":"alpha"},{"value":"beta"}]}"#],
        },
        AbCase {
            name: "while-loop",
            graph_json: WHILE_DIRECT_INDEX_ONLY,
            inputs: &[br#"{"count":3}"#],
        },
        AbCase {
            name: "log-events",
            graph_json: LOG_ALL_LEVELS,
            inputs: &[br#"{"message":"hello"}"#],
        },
        AbCase {
            name: "durable-delay",
            graph_json: DELAY_DYNAMIC,
            inputs: &[br#"{"waitTime":0}"#],
        },
        AbCase {
            name: "durable-agent",
            graph_json: AGENT_RETURN_INPUT,
            inputs: &[br#"{"value":"fresh-agent"}"#],
        },
    ];

    for case in CASES {
        let components_artifact = compile_components_artifact(case.name, case.graph_json);
        let direct_artifact = compile_direct_artifact(&components_dir, case.name, case.graph_json);
        assert_eq!(
            direct_artifact.compiler_mode,
            WorkflowCompilerMode::DirectWasm
        );

        for (input_index, workflow_input) in case.inputs.iter().enumerate() {
            let components_input = components_sdk_input(workflow_input);
            let components = execute_artifact(
                &components_artifact,
                &format!("ab-components-{}-{input_index}", case.name),
                &components_input,
            );
            let direct = execute_artifact(
                &direct_artifact.path,
                &format!("ab-direct-{}-{input_index}", case.name),
                workflow_input,
            );
            assert_success_parity(case.name, input_index, &components, &direct);
        }
    }
}

#[test]
fn direct_wasm_matches_components_cached_durable_agent_checkpoint_replay() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact =
        compile_components_artifact("durable-agent-cached", AGENT_RETURN_INPUT);
    let direct_artifact =
        compile_direct_artifact(&components_dir, "durable-agent-cached", AGENT_RETURN_INPUT);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"value":"fresh-agent"}"#;
    let components_input = components_sdk_input(workflow_input);
    let cached_agent_output = br#""cached-agent""#.to_vec();
    let components = execute_artifact_with_preloaded_checkpoints(
        &components_artifact,
        "ab-components-durable-agent-cached",
        &components_input,
        vec![(AGENT_CACHE_KEY.to_string(), cached_agent_output.clone())],
    );
    let direct = execute_artifact_with_preloaded_checkpoints(
        &direct_artifact.path,
        "ab-direct-durable-agent-cached",
        workflow_input,
        vec![(AGENT_CACHE_KEY.to_string(), cached_agent_output)],
    );

    assert_success_parity("durable-agent-cached", 0, &components, &direct);

    let expected_output = serde_json::json!({ "result": "cached-agent" });
    assert_eq!(components.output_json.as_ref(), Some(&expected_output));
    assert_eq!(direct.output_json.as_ref(), Some(&expected_output));

    let expected_lookup = vec![(AGENT_CACHE_KEY.to_string(), Vec::new())];
    assert_eq!(
        normalized_checkpoints(&components.checkpoints),
        expected_lookup
    );
    assert_eq!(normalized_checkpoints(&direct.checkpoints), expected_lookup);
}

#[test]
fn direct_wasm_matches_components_pause_resume_after_durable_agent_checkpoint() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact =
        compile_components_artifact("durable-agent-pause-resume", AGENT_RETURN_INPUT);
    let direct_artifact = compile_direct_artifact(
        &components_dir,
        "durable-agent-pause-resume",
        AGENT_RETURN_INPUT,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"value":"resume-agent"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components_paused = execute_artifact_with_checkpoint_signal(
        &components_artifact,
        "ab-components-durable-agent-pause",
        &components_input,
        "pause",
    );
    let direct_paused = execute_artifact_with_checkpoint_signal(
        &direct_artifact.path,
        "ab-direct-durable-agent-pause",
        workflow_input,
        "pause",
    );

    assert!(
        components_paused.status_success,
        "components artifact did not suspend cleanly:\n{}",
        components_paused.stderr
    );
    assert!(
        direct_paused.status_success,
        "direct artifact did not suspend cleanly:\n{}",
        direct_paused.stderr
    );
    assert!(
        components_paused.output_json.is_none(),
        "components artifact unexpectedly completed while paused"
    );
    assert!(
        direct_paused.output_json.is_none(),
        "direct artifact unexpectedly completed while paused"
    );
    assert!(
        components_paused.error_json.is_none(),
        "components artifact unexpectedly failed while paused: {:?}",
        components_paused.error_json
    );
    assert!(
        direct_paused.error_json.is_none(),
        "direct artifact unexpectedly failed while paused: {:?}",
        direct_paused.error_json
    );

    let saved_agent_output = br#""resume-agent""#.to_vec();
    let expected_checkpoint_traffic = vec![
        (AGENT_CACHE_KEY.to_string(), Vec::new()),
        (AGENT_CACHE_KEY.to_string(), saved_agent_output.clone()),
    ];
    assert_eq!(
        normalized_checkpoints(&components_paused.checkpoints),
        expected_checkpoint_traffic
    );
    assert_eq!(
        normalized_checkpoints(&direct_paused.checkpoints),
        expected_checkpoint_traffic
    );
    assert_eq!(components_paused.suspended_count, 1);
    assert_eq!(direct_paused.suspended_count, 1);
    let expected_pause_ack = vec![SignalAckRequest {
        signal_type: "pause".to_string(),
    }];
    assert_eq!(components_paused.signal_acks, expected_pause_ack);
    assert_eq!(
        direct_paused.signal_acks,
        vec![SignalAckRequest {
            signal_type: "pause".to_string(),
        }]
    );

    let components_resumed = execute_artifact_with_preloaded_checkpoints(
        &components_artifact,
        "ab-components-durable-agent-resume",
        &components_input,
        vec![(AGENT_CACHE_KEY.to_string(), saved_agent_output.clone())],
    );
    let direct_resumed = execute_artifact_with_preloaded_checkpoints(
        &direct_artifact.path,
        "ab-direct-durable-agent-resume",
        workflow_input,
        vec![(AGENT_CACHE_KEY.to_string(), saved_agent_output)],
    );

    assert_success_parity(
        "durable-agent-pause-resume",
        0,
        &components_resumed,
        &direct_resumed,
    );

    let expected_output = serde_json::json!({ "result": "resume-agent" });
    assert_eq!(
        components_resumed.output_json.as_ref(),
        Some(&expected_output)
    );
    assert_eq!(direct_resumed.output_json.as_ref(), Some(&expected_output));

    let expected_lookup = vec![(AGENT_CACHE_KEY.to_string(), Vec::new())];
    assert_eq!(
        normalized_checkpoints(&components_resumed.checkpoints),
        expected_lookup
    );
    assert_eq!(
        normalized_checkpoints(&direct_resumed.checkpoints),
        expected_lookup
    );
    assert_eq!(components_resumed.suspended_count, 0);
    assert_eq!(direct_resumed.suspended_count, 0);
    assert!(components_resumed.signal_acks.is_empty());
    assert!(direct_resumed.signal_acks.is_empty());
}

#[test]
fn direct_wasm_matches_components_cached_durable_split_checkpoint_replay() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact =
        compile_components_artifact("durable-split-cached", SPLIT_FINISH_WITH_SCHEMAS);
    let direct_artifact = compile_direct_artifact(
        &components_dir,
        "durable-split-cached",
        SPLIT_FINISH_WITH_SCHEMAS,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"items":[{"value":"fresh"}]}"#;
    let components_input = components_sdk_input(workflow_input);
    let cached_split_output =
        br#"{"stepId":"split","stepName":"Unnamed","stepType":"Split","outputs":[{"value":"cached","index":9,"indices":[9]}]}"#.to_vec();
    let components = execute_artifact_with_preloaded_checkpoints(
        &components_artifact,
        "ab-components-durable-split-cached",
        &components_input,
        vec![(SPLIT_CACHE_KEY.to_string(), cached_split_output.clone())],
    );
    let direct = execute_artifact_with_preloaded_checkpoints(
        &direct_artifact.path,
        "ab-direct-durable-split-cached",
        workflow_input,
        vec![(SPLIT_CACHE_KEY.to_string(), cached_split_output)],
    );

    assert_success_parity("durable-split-cached", 0, &components, &direct);

    let expected_output = serde_json::json!({
        "results": [{ "value": "cached", "index": 9, "indices": [9] }]
    });
    assert_eq!(components.output_json.as_ref(), Some(&expected_output));
    assert_eq!(direct.output_json.as_ref(), Some(&expected_output));

    let expected_lookup = vec![(SPLIT_CACHE_KEY.to_string(), Vec::new())];
    assert_eq!(
        normalized_checkpoints(&components.checkpoints),
        expected_lookup
    );
    assert_eq!(normalized_checkpoints(&direct.checkpoints), expected_lookup);
}

#[test]
fn direct_wasm_matches_components_pause_resume_after_durable_split_checkpoint() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact =
        compile_components_artifact("durable-split-pause-resume", SPLIT_FINISH_WITH_SCHEMAS);
    let direct_artifact = compile_direct_artifact(
        &components_dir,
        "durable-split-pause-resume",
        SPLIT_FINISH_WITH_SCHEMAS,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"items":[{"value":"resume-split"}]}"#;
    let components_input = components_sdk_input(workflow_input);
    let components_paused = execute_artifact_with_checkpoint_signal(
        &components_artifact,
        "ab-components-durable-split-pause",
        &components_input,
        "pause",
    );
    let direct_paused = execute_artifact_with_checkpoint_signal(
        &direct_artifact.path,
        "ab-direct-durable-split-pause",
        workflow_input,
        "pause",
    );

    assert!(
        components_paused.status_success,
        "components artifact did not suspend cleanly:\n{}",
        components_paused.stderr
    );
    assert!(
        direct_paused.status_success,
        "direct artifact did not suspend cleanly:\n{}",
        direct_paused.stderr
    );
    assert!(
        components_paused.output_json.is_none(),
        "components artifact unexpectedly completed while paused"
    );
    assert!(
        direct_paused.output_json.is_none(),
        "direct artifact unexpectedly completed while paused"
    );
    assert!(
        components_paused.error_json.is_none(),
        "components artifact unexpectedly failed while paused: {:?}",
        components_paused.error_json
    );
    assert!(
        direct_paused.error_json.is_none(),
        "direct artifact unexpectedly failed while paused: {:?}",
        direct_paused.error_json
    );

    let components_checkpoint_traffic = normalized_checkpoints(&components_paused.checkpoints);
    let direct_checkpoint_traffic = normalized_checkpoints(&direct_paused.checkpoints);
    assert_eq!(components_checkpoint_traffic, direct_checkpoint_traffic);
    assert_eq!(components_checkpoint_traffic.len(), 2);
    assert_eq!(
        components_checkpoint_traffic[0],
        (SPLIT_CACHE_KEY.to_string(), Vec::new())
    );
    assert_eq!(
        components_checkpoint_traffic[1].0,
        SPLIT_CACHE_KEY.to_string()
    );
    assert!(
        !components_checkpoint_traffic[1].1.is_empty(),
        "paused Split run did not save checkpoint state"
    );
    assert_eq!(components_paused.suspended_count, 1);
    assert_eq!(direct_paused.suspended_count, 1);
    let expected_pause_ack = vec![SignalAckRequest {
        signal_type: "pause".to_string(),
    }];
    assert_eq!(components_paused.signal_acks, expected_pause_ack);
    assert_eq!(
        direct_paused.signal_acks,
        vec![SignalAckRequest {
            signal_type: "pause".to_string(),
        }]
    );

    let saved_split_result = components_checkpoint_traffic[1].1.clone();
    let components_resumed = execute_artifact_with_preloaded_checkpoints(
        &components_artifact,
        "ab-components-durable-split-resume",
        &components_input,
        vec![(SPLIT_CACHE_KEY.to_string(), saved_split_result.clone())],
    );
    let direct_resumed = execute_artifact_with_preloaded_checkpoints(
        &direct_artifact.path,
        "ab-direct-durable-split-resume",
        workflow_input,
        vec![(SPLIT_CACHE_KEY.to_string(), saved_split_result)],
    );

    assert_success_parity(
        "durable-split-pause-resume",
        0,
        &components_resumed,
        &direct_resumed,
    );

    let expected_output = serde_json::json!({
        "results": [{ "value": "resume-split", "index": 0, "indices": [0] }]
    });
    assert_eq!(
        components_resumed.output_json.as_ref(),
        Some(&expected_output)
    );
    assert_eq!(direct_resumed.output_json.as_ref(), Some(&expected_output));

    let expected_lookup = vec![(SPLIT_CACHE_KEY.to_string(), Vec::new())];
    assert_eq!(
        normalized_checkpoints(&components_resumed.checkpoints),
        expected_lookup
    );
    assert_eq!(
        normalized_checkpoints(&direct_resumed.checkpoints),
        expected_lookup
    );
    assert_eq!(components_resumed.suspended_count, 0);
    assert_eq!(direct_resumed.suspended_count, 0);
    assert!(components_resumed.signal_acks.is_empty());
    assert!(direct_resumed.signal_acks.is_empty());
}

#[test]
fn direct_wasm_matches_components_failure_for_error_fixture() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact = compile_components_artifact("error", ERROR_DIRECT_SIMPLE);
    let direct_artifact = compile_direct_artifact(&components_dir, "error", ERROR_DIRECT_SIMPLE);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"requestId":"req-123"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-error",
        &components_input,
    );
    let direct = execute_artifact(&direct_artifact.path, "ab-direct-error", workflow_input);

    assert_failure_parity("error", 0, &components, &direct);
}
