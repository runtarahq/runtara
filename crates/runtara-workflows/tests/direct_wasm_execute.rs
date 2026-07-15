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
    RuntimeBinding, WorkflowAbi, compile_direct_workflow, compile_direct_workflow_composed,
    compile_direct_workflow_composed_configured, compile_direct_workflow_composed_with_binding,
    compose_direct_workflow, emit_direct_component_artifacts_with_binding,
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
const AGENT_EDGE_CONDITION: &str = include_str!("fixtures/agent_edge_condition.json");
const WAIT_TIMEOUT_ON_ERROR: &str = include_str!("fixtures/wait_timeout_on_error.json");
const WAIT_DELAY_FINISH: &str = include_str!("fixtures/wait_delay_finish.json");
const WAIT_WAIT_FINISH: &str = include_str!("fixtures/wait_wait_finish.json");
const WHILE_DIRECT_INDEX_ONLY: &str = include_str!("fixtures/while_direct_index_only.json");
const WHILE_TIMEOUT: &str = include_str!("fixtures/while_timeout.json");
const SPLIT_TIMEOUT: &str = include_str!("fixtures/split_timeout.json");
const SPLIT_WORKFLOW: &str = include_str!("fixtures/split_workflow.json");
const CONDITIONAL_QUERY_ONLY_OPERATOR: &str =
    include_str!("fixtures/conditional_query_only_operator.json");
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

/// SYN-448: a single Finish whose mappings index an array with Python-style
/// negative indices. `-1` is the last element, `-3` the first; positive indices
/// are unchanged; an out-of-range negative falls through to the mapping default.
/// Proves the reference resolver honors negative indexing in the compiled +
/// executed WASM runtime, not just in host-side unit tests.
const NEGATIVE_INDEX_REFERENCE: &str = r#"{
  "steps": {
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "last":      { "valueType": "reference", "value": "data.items.-1" },
        "second":    { "valueType": "reference", "value": "data.items.-2" },
        "first_neg": { "valueType": "reference", "value": "data.items.-3" },
        "first_pos": { "valueType": "reference", "value": "data.items.0" },
        "oob":       { "valueType": "reference", "value": "data.items.-9", "default": "fallback" }
      }
    }
  },
  "entryPoint": "finish",
  "executionPlan": [],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}"#;

/// SYN-449: a `template` mapping using the `tojson` filter, which is only
/// available when minijinja's `json` feature is enabled. Proves the filter works
/// in the compiled + executed WASM mapping engine, not just host-side unit tests.
const TEMPLATE_TOJSON_FILTER: &str = r#"{
  "steps": {
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "json_str": { "valueType": "template", "value": "{{ data.obj | tojson }}" }
      }
    }
  },
  "entryPoint": "finish",
  "executionPlan": [],
  "variables": {},
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

/// Cross-linked fan-out inside a Conditional branch (the distilled
/// CategorizeViaUnspsc miss-path): `gate` fans out to `left` and `right`, and
/// `after_left` — downstream of `left` — consumes `right`'s output. The region's
/// topological order must run `right` before `after_left`; the per-fan-out merge
/// recursion this replaced ran branch 0's whole chain first, so the cross
/// reference resolved to null (and a failure there meant `right` never ran at
/// all — the reported "second fan-out edge silently dropped").
const FANOUT_CROSS_BRANCH_REFERENCE: &str = r#"{
  "durable": false,
  "steps": {
    "cond": {
      "stepType": "Conditional", "id": "cond",
      "condition": {
        "type": "operation", "op": "EQ",
        "arguments": [
          {"value": "x", "valueType": "immediate"},
          {"value": "y", "valueType": "immediate"}
        ]
      }
    },
    "hit": {
      "stepType": "Agent", "id": "hit", "name": "Hit",
      "agentId": "utils", "capabilityId": "return-input",
      "inputMapping": {"value": {"valueType": "immediate", "value": "H"}}
    },
    "gate": {
      "stepType": "Agent", "id": "gate", "name": "Gate",
      "agentId": "utils", "capabilityId": "return-input",
      "inputMapping": {"value": {"valueType": "immediate", "value": "G"}}
    },
    "left": {
      "stepType": "Agent", "id": "left", "name": "Left",
      "agentId": "utils", "capabilityId": "return-input",
      "inputMapping": {"value": {"valueType": "immediate", "value": "L"}}
    },
    "right": {
      "stepType": "Agent", "id": "right", "name": "Right",
      "agentId": "utils", "capabilityId": "return-input",
      "inputMapping": {"value": {"valueType": "immediate", "value": "R"}}
    },
    "after_left": {
      "stepType": "Agent", "id": "after_left", "name": "After Left",
      "agentId": "utils", "capabilityId": "return-input",
      "inputMapping": {"value": {"valueType": "reference", "value": "steps.right.outputs"}}
    },
    "finish": {
      "stepType": "Finish", "id": "finish",
      "inputMapping": {
        "crossed": {"valueType": "reference", "value": "steps.after_left.outputs"},
        "left":    {"valueType": "reference", "value": "steps.left.outputs"},
        "right":   {"valueType": "reference", "value": "steps.right.outputs"}
      }
    }
  },
  "entryPoint": "cond",
  "executionPlan": [
    { "fromStep": "cond", "label": "true",  "toStep": "hit" },
    { "fromStep": "cond", "label": "false", "toStep": "gate" },
    { "fromStep": "gate", "toStep": "left" },
    { "fromStep": "gate", "toStep": "right" },
    { "fromStep": "left", "toStep": "after_left" },
    { "fromStep": "after_left", "toStep": "finish" },
    { "fromStep": "right", "toStep": "finish" },
    { "fromStep": "hit", "toStep": "finish" }
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
    /// LLM-proxy request envelopes the workflow sent (one per model call).
    llm_requests: Vec<Value>,
    /// Raw-SQL request paths the workflow sent (one per attempt — retries
    /// included), in order.
    sql_requests: Vec<String>,
    /// Number of custom-signal polls the mock answered with a signal — a
    /// replayed wait re-polls, so this is > the number of waits after a resume.
    custom_signal_polls: u32,
    status_success: bool,
    stderr: String,
    /// Peak guest linear memory observed by the embedded executor's limiter, when
    /// the embedded path ran it. `None` under the CLI executor (no limiter hook).
    memory_peak_bytes: Option<u64>,
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
    /// Scripted LLM-proxy responses, served front-to-back to POST /llm-proxy.
    /// Each entry is the proxy envelope `{status, headers, body}` the
    /// workflow's `call_agent()` will deserialize into an HttpResponse.
    llm_responses: Mutex<Vec<Value>>,
    /// Proxy request envelopes received on POST /llm-proxy, in order.
    llm_requests: Mutex<Vec<Value>>,
    /// Scripted `(status, body)` responses for the object-model raw-SQL
    /// routes, served front-to-back. Empty script → generic success, so
    /// unrelated tests are unaffected.
    sql_responses: Mutex<Vec<(u16, Value)>>,
    /// Paths of raw-SQL requests received, in order — retry counting.
    sql_requests: Mutex<Vec<String>>,
    /// Payloads served for custom-signal polls (`GET signals/{id}`), modeling
    /// the pending-signal row. Served **non-destructively** (peeked, never
    /// removed) so a replayed `WaitForSignal` re-reads the same signal — the
    /// core `take_pending_custom_signal` is likewise a non-destructive read.
    /// The first entry answers every poll; empty → no signal (the wait keeps
    /// polling), so a test that arms no signal would hang by design.
    custom_signals: Mutex<Vec<Value>>,
    /// Count of custom-signal polls served with a signal — lets a test assert
    /// the wait re-polled on replay.
    custom_signal_polls: Mutex<u32>,
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn shared_components_dir() -> PathBuf {
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
    assert!(
        missing.is_empty(),
        "direct-wasm-integration-tests requires staged shared components: {missing:?}; run scripts/build-agent-components.sh"
    );
    let stdlib_wasm = dir.join("runtara_workflow_stdlib.wasm");
    let stdlib_bytes = std::fs::read(&stdlib_wasm)
        .unwrap_or_else(|error| panic!("read required {stdlib_wasm:?}: {error}"));
    let required_stdlib_markers: &[&[u8]] = &[
        b"split-cache-key",
        b"embed-workflow-cache-key",
        b"embed-workflow-variables",
        b"embed-workflow-result",
        b"embed-workflow-output-from-result",
        b"embed-workflow-error",
    ];
    assert!(
        required_stdlib_markers.iter().all(|marker| {
            stdlib_bytes
                .windows(marker.len())
                .any(|window| window == *marker)
        }),
        "required shared workflow stdlib is stale: {stdlib_wasm:?}; run scripts/build-agent-components.sh"
    );
    dir
}

/// Dev-tool lookup for the opt-in CLI reference mode: honor `WASMTIME_PATH`,
/// then `~/.wasmtime/bin/wasmtime`, then PATH.
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

// Serve one HTTP request from a *persistent* connection reader. The reader is
// owned by the connection loop and reused across requests, NOT recreated here:
// a `BufReader` reads ahead in blocks, so it routinely pulls the first bytes of
// the *next* request past the current request's body. A per-request reader
// (the previous design) discarded that read-ahead when it was dropped, so the
// next request on a reused keep-alive connection began mid-stream — a desync the
// client surfaced as `HttpProtocolError`. It only bit under load, when the SDK's
// next request had already arrived by the time we read this one's body — i.e. on
// long, many-request runs (AiAgent loops). Returns `Ok(true)` to keep the
// connection, `Ok(false)`/`Err` to close it.
fn handle_request(
    reader: &mut BufReader<std::net::TcpStream>,
    sink: &mpsc::Sender<CapturedMessage>,
    server_state: &ServerState,
    workflow_input: &[u8],
) -> std::io::Result<bool> {
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
        read_chunked_body(reader)?
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
    // Write through the underlying stream. The BufReader only buffers reads, so
    // its retained read-ahead survives across requests (full-duplex socket).
    let stream = reader.get_mut();
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

    // Hermetic LLM stub: `call_agent()` forwards provider requests here when
    // RUNTARA_HTTP_PROXY_URL points at the mock server. Pop the next scripted
    // proxy envelope; running out of script is a test bug, surfaced as 599
    // so the workflow fails loudly instead of hanging on `{success: true}`.
    if method == "POST" && path == "/llm-proxy" {
        let envelope: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        server_state
            .llm_requests
            .lock()
            .expect("llm_requests lock")
            .push(envelope);
        let mut responses = server_state
            .llm_responses
            .lock()
            .expect("llm_responses lock");
        if responses.is_empty() {
            return (
                200,
                serde_json::json!({
                    "status": 599,
                    "headers": {},
                    "body": {"error": "llm stub script exhausted"}
                }),
            );
        }
        return (200, responses.remove(0));
    }

    // Raw-SQL stub for the object-model query-sql / execute-sql capabilities:
    // record the request (retry-count assertions), then pop the next scripted
    // (status, body). An empty script answers success.
    if method == "POST" && path.contains("/object-model/sql/") {
        server_state
            .sql_requests
            .lock()
            .expect("sql_requests lock")
            .push(path.to_string());
        let mut responses = server_state
            .sql_responses
            .lock()
            .expect("sql_responses lock");
        if responses.is_empty() {
            return (
                200,
                serde_json::json!({"success": true, "rows": [], "rowCount": 0, "rowsAffected": 1}),
            );
        }
        return responses.remove(0);
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
            // Lifecycle-signal poll (WaitForSignal loop's `check_signals`). No
            // drain injected in these tests → no pending lifecycle signal.
            ("GET", "signals") => {
                return (
                    200,
                    serde_json::json!({"signal": null, "custom_signal": null}),
                );
            }
            // Custom-signal poll (`GET signals/{signal_id}`). The signal id is a
            // single percent-encoded path segment, so it lands here as
            // `signals/<encoded>`; a fixture has one wait per id, so we ignore
            // the exact id and serve the armed payload. Non-destructive: peek
            // the front and leave it, so a replayed wait re-reads it.
            ("GET", ep) if ep.starts_with("signals/") => {
                let custom = server_state
                    .custom_signals
                    .lock()
                    .expect("custom_signals lock")
                    .first()
                    .cloned();
                let custom_signal = custom.map(|payload| {
                    *server_state
                        .custom_signal_polls
                        .lock()
                        .expect("custom_signal_polls lock") += 1;
                    let payload_b64 = base64::engine::general_purpose::STANDARD
                        .encode(serde_json::to_vec(&payload).expect("payload serializes"));
                    serde_json::json!({"checkpoint_id": "wait", "payload": payload_b64})
                });
                return (
                    200,
                    serde_json::json!({"signal": null, "custom_signal": custom_signal}),
                );
            }
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
            Ok((stream, _)) => {
                let sink = sink.clone();
                let server_state = server_state.clone();
                let workflow_input = workflow_input.clone();
                thread::spawn(move || {
                    // Accepted sockets can inherit the listener's non-blocking flag
                    // (macOS); force blocking + a timeout so request parsing blocks
                    // for the next keep-alive request rather than erroring, and a
                    // dead peer eventually frees the thread.
                    stream.set_nonblocking(false).ok();
                    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
                    stream.set_write_timeout(Some(Duration::from_secs(10))).ok();
                    // ONE reader for the whole connection: its read-ahead buffer
                    // must persist across requests (see `handle_request`).
                    let mut reader = BufReader::new(stream);
                    while let Ok(true) =
                        handle_request(&mut reader, &sink, &server_state, workflow_input.as_slice())
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

fn direct_e2e_components_dir() -> PathBuf {
    // Composition is in-process via the `wac-graph` crate (see
    // `direct_wasm/compile.rs`) — the `wac` CLI is never invoked, so it must
    // not be required here. A stale `tool_installed("wac")` guard made the
    // whole suite panic in CI environments that stage the components but don't
    // install the (unused) CLI.
    assert!(
        embedded_executor_mode() || wasmtime_installed(),
        "direct-wasm-integration-tests in CLI mode requires wasmtime"
    );
    shared_components_dir()
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
        Vec::new(),
    )
}

/// Run a workflow whose AiAgent steps call the scripted LLM stub. Each script
/// entry is a proxy envelope `{status, headers, body}` served in order to the
/// workflow's model calls; the returned run carries the recorded requests.
fn run_direct_workflow_with_llm_script(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    workflow_input: &[u8],
    llm_script: Vec<Value>,
) -> CapturedRun {
    run_direct_workflow_capture_with_preloaded_checkpoints(
        components_dir,
        workflow_id,
        graph_json,
        workflow_input,
        false,
        Vec::new(),
        llm_script,
    )
}

/// Run a `WaitForSignal` workflow against the mock, arming a non-destructive
/// custom-signal payload the wait(s) will consume. `preloaded_checkpoints`
/// simulates a drain/resume: pass a prior run's captured checkpoints to replay
/// the instance from the entry point with its durable state already present.
fn run_wait_workflow(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    workflow_input: &[u8],
    preloaded_checkpoints: Vec<(String, Vec<u8>)>,
    custom_signals: Vec<Value>,
) -> CapturedRun {
    run_direct_workflow_capture_full_sql(
        components_dir,
        workflow_id,
        graph_json,
        workflow_input,
        false,
        preloaded_checkpoints,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        custom_signals,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_direct_workflow_capture_with_preloaded_checkpoints(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    workflow_input: &[u8],
    track_events: bool,
    preloaded_checkpoints: Vec<(String, Vec<u8>)>,
    llm_script: Vec<Value>,
) -> CapturedRun {
    run_direct_workflow_capture_full(
        components_dir,
        workflow_id,
        graph_json,
        workflow_input,
        track_events,
        preloaded_checkpoints,
        llm_script,
        Vec::new(),
    )
}

#[allow(clippy::too_many_arguments)]
fn run_direct_workflow_capture_full(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    workflow_input: &[u8],
    track_events: bool,
    preloaded_checkpoints: Vec<(String, Vec<u8>)>,
    llm_script: Vec<Value>,
    extra_env: Vec<(String, String)>,
) -> CapturedRun {
    run_direct_workflow_capture_full_sql(
        components_dir,
        workflow_id,
        graph_json,
        workflow_input,
        track_events,
        preloaded_checkpoints,
        llm_script,
        extra_env,
        Vec::new(),
        Vec::new(),
    )
}

/// `run_direct_workflow_capture_full` plus a scripted `(status, body)` queue
/// for the object-model raw-SQL routes — retry-semantics tests count attempts
/// via `CapturedRun::sql_requests`.
#[allow(clippy::too_many_arguments)]
fn run_direct_workflow_capture_full_sql(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    workflow_input: &[u8],
    track_events: bool,
    preloaded_checkpoints: Vec<(String, Vec<u8>)>,
    llm_script: Vec<Value>,
    extra_env: Vec<(String, String)>,
    sql_script: Vec<(u16, Value)>,
    custom_signals: Vec<Value>,
) -> CapturedRun {
    let first = run_direct_workflow_capture_attempt(
        components_dir,
        workflow_id,
        graph_json,
        workflow_input,
        track_events,
        preloaded_checkpoints.clone(),
        llm_script.clone(),
        extra_env.clone(),
        sql_script.clone(),
        custom_signals.clone(),
    );
    // Under full-suite parallel load (16 threads × wasmtime spawns + ephemeral
    // TCP listeners) a run occasionally dies before reaching the mock runtime
    // at all: non-zero exit, EMPTY stderr, and zero captured traffic. That
    // signature is infrastructure (spawn/connect), not workflow behavior —
    // retry once so a 1-in-N-suites flake doesn't fail the suite. Real
    // failures always leave stderr or a /failed capture and are NOT retried.
    let infra_flake = !first.status_success
        && first.stderr.trim().is_empty()
        && first.output_json.is_none()
        && first.error_json.is_none()
        && first.events.is_empty()
        && first.checkpoints.is_empty();
    if !infra_flake {
        return first;
    }
    eprintln!("retrying '{workflow_id}': wasmtime spawn/connect flake (empty stderr, no traffic)");
    run_direct_workflow_capture_attempt(
        components_dir,
        workflow_id,
        graph_json,
        workflow_input,
        track_events,
        preloaded_checkpoints,
        llm_script,
        extra_env,
        sql_script,
        custom_signals,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_direct_workflow_capture_attempt(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    workflow_input: &[u8],
    track_events: bool,
    preloaded_checkpoints: Vec<(String, Vec<u8>)>,
    llm_script: Vec<Value>,
    extra_env: Vec<(String, String)>,
    sql_script: Vec<(u16, Value)>,
    custom_signals: Vec<Value>,
) -> CapturedRun {
    let temp = tempfile::tempdir().expect("tempdir");
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    let binding = runtime_binding_mode();
    let abi = workflow_abi_mode();
    let compiled = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: workflow_id.to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: temp.path().to_path_buf(),
            track_events,
            agent_catalog: None,
        },
        components_dir,
        binding,
        abi,
        // The battery axis exercises the blocking durable-sleep path; the
        // store-freeing lowering has its own dedicated suspend/resume test.
        false,
        // Runtime import kept — omit-runtime has its own dedicated test.
        false,
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
        llm_responses: Mutex::new(llm_script),
        llm_requests: Mutex::new(Vec::new()),
        sql_responses: Mutex::new(sql_script),
        sql_requests: Mutex::new(Vec::new()),
        custom_signals: Mutex::new(custom_signals),
        custom_signal_polls: Mutex::new(0),
    });
    let server_state_for_assertions = server_state.clone();
    let capture_tx_for_host = capture_tx.clone();
    let workflow_input_for_host = Arc::clone(&workflow_input);
    let server_handle =
        thread::spawn(move || serve(listener, capture_tx, server_state, stop_rx, workflow_input));

    // Env contract shared by both execution paths. The object-model URL keeps
    // that traffic hermetic: its default base URL points at a live local
    // environment (127.0.0.1:7002); route it to the mock, whose generic
    // `{"success": true}` fallback answers internal calls.
    let mut env_pairs: Vec<(String, String)> = vec![
        ("RUNTARA_HTTP_URL".into(), format!("http://{addr}")),
        (
            "RUNTARA_HTTP_PROXY_URL".into(),
            format!("http://{addr}/llm-proxy"),
        ),
        (
            "RUNTARA_OBJECT_MODEL_URL".into(),
            format!("http://{addr}/object-model"),
        ),
        ("RUNTARA_SERVER_ADDR".into(), addr.to_string()),
        ("RUNTARA_INSTANCE_ID".into(), workflow_id.to_string()),
        ("RUNTARA_TENANT_ID".into(), "direct-wasm-execute".into()),
        ("RUST_LOG".into(), "warn".into()),
    ];
    env_pairs.extend(extra_env.iter().cloned());

    // Under HostImport, the runtime interface is served by the capturing host
    // (same ServerState + capture sink as the mock server, so assertions see
    // one uniform CapturedRun shape); the mock keeps serving the wasi:http
    // traffic that stays real under both bindings (LLM proxy, object-model).
    let runtime_host: Option<Arc<dyn runtara_component_host::runtime_host::RuntimeHost>> =
        (binding == RuntimeBinding::HostImport).then(|| {
            let debug_mode = env_pairs
                .iter()
                .any(|(key, value)| key == "DEBUG_MODE" && value == "true");
            Arc::new(CapturingRuntimeHost {
                instance_id: workflow_id.to_string(),
                debug_mode,
                input: Arc::clone(&workflow_input_for_host),
                sink: Mutex::new(capture_tx_for_host.clone()),
                state: server_state_for_assertions.clone(),
            }) as Arc<dyn runtara_component_host::runtime_host::RuntimeHost>
        });

    let (status_success, stderr, memory_peak_bytes) = if !embedded_executor_mode() {
        let (ok, err) = execute_via_cli(&compiled.wasm_path, &env_pairs);
        (ok, err, None)
    } else if abi == WorkflowAbi::InvokeHostImports {
        execute_via_embedded_invoke(
            &compiled.wasm_path,
            &env_pairs,
            runtime_host.expect("invoke ABI requires the capturing host"),
            workflow_input_for_host.as_ref().clone(),
        )
    } else {
        execute_via_embedded(&compiled.wasm_path, &env_pairs, runtime_host)
    };
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
    let llm_requests = server_state_for_assertions
        .llm_requests
        .lock()
        .expect("llm_requests lock")
        .clone();
    let sql_requests = server_state_for_assertions
        .sql_requests
        .lock()
        .expect("sql_requests lock")
        .clone();
    let custom_signal_polls = *server_state_for_assertions
        .custom_signal_polls
        .lock()
        .expect("custom_signal_polls lock");
    CapturedRun {
        output_json,
        error_json,
        events,
        sleeps,
        checkpoints,
        llm_requests,
        sql_requests,
        custom_signal_polls,
        status_success,
        stderr,
        memory_peak_bytes,
    }
}

/// Battery-wide executor selection. The in-process WorkflowExecutor is the
/// default (it is the only production runner); `RUNTARA_DIRECT_WASM_EXECUTOR=cli`
/// opts into the reference wasmtime CLI for A/B cross-checks of the composed
/// component against the upstream runtime.
fn embedded_executor_mode() -> bool {
    std::env::var("RUNTARA_DIRECT_WASM_EXECUTOR").as_deref() != Ok("cli")
}

/// Battery-wide runtime-binding selection. HostImport (the production
/// default) satisfies the runtime interface natively via a capturing
/// RuntimeHost; `RUNTARA_DIRECT_RUNTIME_BINDING=composed` re-runs the whole
/// battery through the legacy composed runtime + mock HTTP core — the
/// binding-differential axis. The CLI executor always forces Composed (the
/// wasmtime CLI has no way to satisfy host imports).
fn runtime_binding_mode() -> RuntimeBinding {
    if !embedded_executor_mode() {
        return RuntimeBinding::Composed;
    }
    match std::env::var("RUNTARA_DIRECT_RUNTIME_BINDING").as_deref() {
        Ok("composed") => RuntimeBinding::Composed,
        _ => RuntimeBinding::HostImport,
    }
}

/// Battery-wide export-shape selection, mirroring the production default:
/// the invoke export (input as the call argument, terminal result in-band).
/// `RUNTARA_DIRECT_WORKFLOW_ABI=cli-run` re-runs the whole battery through
/// the legacy shape — the ABI-differential axis. The CLI executor and the
/// Composed binding force the legacy shape (neither can drive host imports).
fn workflow_abi_mode() -> WorkflowAbi {
    if !embedded_executor_mode() || runtime_binding_mode() == RuntimeBinding::Composed {
        return WorkflowAbi::CliRunHttp;
    }
    match std::env::var("RUNTARA_DIRECT_WORKFLOW_ABI").as_deref() {
        Ok("cli-run") => WorkflowAbi::CliRunHttp,
        _ => WorkflowAbi::InvokeHostImports,
    }
}

/// RuntimeHost that mirrors the mock core server route-for-route, sharing the
/// SAME `ServerState` and capture sink — so a HostImport run produces the
/// exact `CapturedRun` shape a Composed run produces over HTTP, and every
/// existing assertion applies unchanged to both bindings.
struct CapturingRuntimeHost {
    instance_id: String,
    debug_mode: bool,
    input: Arc<Vec<u8>>,
    /// `mpsc::Sender` is `!Sync`; the host must be `Sync`.
    sink: Mutex<mpsc::Sender<CapturedMessage>>,
    state: Arc<ServerState>,
}

impl CapturingRuntimeHost {
    fn send(&self, message: CapturedMessage) {
        let _ = self.sink.lock().expect("capture sink lock").send(message);
    }
}

#[async_trait::async_trait]
impl runtara_component_host::runtime_host::RuntimeHost for CapturingRuntimeHost {
    async fn load_input(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(Some(self.input.as_ref().clone()))
    }
    fn instance_id(&self) -> Result<String, String> {
        Ok(self.instance_id.clone())
    }
    async fn complete(&self, output: Vec<u8>) -> Result<(), String> {
        // Mirror capture_completed: only JSON outputs are recorded.
        if let Ok(output_json) = serde_json::from_slice::<Value>(&output) {
            self.send(CapturedMessage::Completed(Completed { output_json }));
        }
        Ok(())
    }
    async fn fail(&self, error: Vec<u8>) -> Result<(), String> {
        // Mirror capture_failed: JSON errors parse, everything else is a string.
        let error_str = String::from_utf8_lossy(&error);
        let error_json = serde_json::from_str::<Value>(&error_str)
            .unwrap_or_else(|_| Value::String(error_str.clone().into_owned()));
        self.send(CapturedMessage::Failed(Failed { error_json }));
        Ok(())
    }
    async fn custom_event(&self, kind: String, payload: Vec<u8>) -> Result<(), String> {
        // Mirror capture_event: only custom events with JSON payloads are
        // recorded (every guest custom-event is event_type=custom over HTTP).
        if let Ok(payload_json) = serde_json::from_slice::<Value>(&payload) {
            self.send(CapturedMessage::Event(RuntimeEvent {
                subtype: kind,
                payload_json,
            }));
        }
        Ok(())
    }
    fn debug_mode_enabled(&self) -> Result<bool, String> {
        Ok(self.debug_mode)
    }
    async fn breakpoint_pause(&self) -> Result<(), String> {
        // The mock records nothing for signals/ack + /suspended.
        Ok(())
    }
    async fn heartbeat(&self) -> Result<(), String> {
        // Mirror: heartbeat events are filtered out by capture_event.
        Ok(())
    }
    async fn is_cancelled(&self) -> Result<bool, String> {
        Ok(false)
    }
    async fn check_signals(&self) -> Result<bool, String> {
        // Mirror GET /signals: no drain is injected in these tests.
        Ok(false)
    }
    async fn poll_custom_signal(&self, _checkpoint_id: String) -> Result<Option<Vec<u8>>, String> {
        // Mirror GET signals/{id}: peek the front NON-destructively (a
        // replayed wait re-reads the same signal) and count answered polls.
        let custom = self
            .state
            .custom_signals
            .lock()
            .expect("custom_signals lock")
            .first()
            .cloned();
        Ok(custom.map(|payload| {
            *self
                .state
                .custom_signal_polls
                .lock()
                .expect("custom_signal_polls lock") += 1;
            serde_json::to_vec(&payload).expect("payload serializes")
        }))
    }
    async fn get_checkpoint(&self, checkpoint_id: String) -> Result<Option<Vec<u8>>, String> {
        // The HTTP SDK routes get_checkpoint through POST /checkpoint with
        // empty state, so the mock records an empty-state Checkpoint capture;
        // mirror both the capture and the read-only lookup.
        self.send(CapturedMessage::Checkpoint(CheckpointRequest {
            checkpoint_id: checkpoint_id.clone(),
            state: Vec::new(),
        }));
        Ok(self
            .state
            .checkpoints
            .lock()
            .expect("checkpoint state lock")
            .get(&checkpoint_id)
            .cloned())
    }
    async fn checkpoint(
        &self,
        checkpoint_id: String,
        state: Vec<u8>,
    ) -> Result<runtara_component_host::runtime_host::RuntimeCheckpointResult, String> {
        // Mirror checkpoint_response: always capture, hit returns the stored
        // state, miss saves only non-empty state (the read-only-probe rule).
        self.send(CapturedMessage::Checkpoint(CheckpointRequest {
            checkpoint_id: checkpoint_id.clone(),
            state: state.clone(),
        }));
        let mut checkpoints = self
            .state
            .checkpoints
            .lock()
            .expect("checkpoint state lock");
        if let Some(existing) = checkpoints.get(&checkpoint_id) {
            return Ok(
                runtara_component_host::runtime_host::RuntimeCheckpointResult {
                    found: true,
                    state: existing.clone(),
                    pending_signal: None,
                    custom_signal: None,
                },
            );
        }
        if !state.is_empty() {
            checkpoints.insert(checkpoint_id, state);
        }
        Ok(
            runtara_component_host::runtime_host::RuntimeCheckpointResult {
                found: false,
                state: Vec::new(),
                pending_signal: None,
                custom_signal: None,
            },
        )
    }
    async fn handle_checkpoint_signal(&self, _signal_type: String) -> Result<bool, String> {
        Ok(false)
    }
    async fn record_retry_attempt(
        &self,
        _checkpoint_id: String,
        _attempt_number: u32,
        _error_message: Option<String>,
    ) -> Result<(), String> {
        // Mirror: POST /retry falls to the mock's generic success catch-all.
        Ok(())
    }
    async fn durable_sleep_checkpoint(
        &self,
        checkpoint_id: String,
        state: Vec<u8>,
        ms: u64,
    ) -> Result<(), String> {
        // Mirror POST /sleep: capture only — the mock neither persists the
        // checkpoint nor sleeps, keeping delay-heavy fixtures fast.
        self.send(CapturedMessage::Sleep(SleepRequest {
            checkpoint_id,
            duration_ms: ms,
            state,
        }));
        Ok(())
    }
}

/// CLI path: spawn `wasmtime run --wasi http` exactly as `WasmRunner` does.
fn execute_via_cli(wasm_path: &Path, env_pairs: &[(String, String)]) -> (bool, String) {
    let mut command = Command::new(wasmtime_binary());
    command
        .arg("run")
        .arg("--wasi")
        .arg("http")
        .arg("--wasi")
        .arg("inherit-network");
    for (key, value) in env_pairs {
        command.arg("--env").arg(format!("{key}={value}"));
    }
    let output = command
        .arg(wasm_path)
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .output()
        .expect("spawn wasmtime");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Embedded path: same component, same env, executed in-process. Returns the
/// status, the failure reason (empty on success/guest-error), and the exact guest
/// linear-memory peak from the executor's limiter.
/// Invoke-ABI path: same env and limits, but input travels as the call
/// argument and the terminal result is the lifted return value. The captures
/// keep flowing through the additive complete/fail recordings the
/// CapturingRuntimeHost already mirrors — the CapturedRun shape is identical
/// across all three execution paths.
fn execute_via_embedded_invoke(
    wasm_path: &Path,
    env_pairs: &[(String, String)],
    runtime_host: Arc<dyn runtara_component_host::runtime_host::RuntimeHost>,
    input: Vec<u8>,
) -> (bool, String, Option<u64>) {
    let executor = embedded_executor();
    let mut limits = runtara_component_host::WorkflowLimits::default();
    if let Some(max) = env_pairs
        .iter()
        .find(|(key, _)| key == "RUNTARA_INSTANCE_MEMORY_MAX_BYTES")
        .and_then(|(_, value)| value.parse::<usize>().ok())
    {
        limits.max_memory_bytes = max;
    }
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let result = runtime.block_on(async {
        let pre = executor
            .load_instance_pre(wasm_path)
            .await
            .expect("load invoke-shaped workflow component");
        executor
            .execute_invoke(
                &pre,
                runtara_component_host::WorkflowRunSpec {
                    env: env_pairs.iter().cloned().collect(),
                    stderr: None,
                    timeout: Duration::from_secs(300),
                    cancel: None,
                    limits,
                    runtime: Some(runtime_host),
                },
                input,
            )
            .await
    });
    let peak = Some(result.memory_peak_bytes);
    match result.exit {
        runtara_component_host::InvokeExit::Completed(_) => (true, String::new(), peak),
        // The additive fail recording carries the error payload for the
        // assertions; status mirrors the legacy non-zero exit.
        runtara_component_host::InvokeExit::Failed(_) => (false, String::new(), peak),
        // A lifecycle suspension is the clean exit the legacy run reported as
        // Ok — the suspended status was recorded host-side by the ack.
        runtara_component_host::InvokeExit::Suspended(_) => (true, String::new(), peak),
        runtara_component_host::InvokeExit::Trapped { reason } => (false, reason, peak),
        runtara_component_host::InvokeExit::Timeout => (false, "invoke timeout".to_string(), peak),
        runtara_component_host::InvokeExit::Cancelled => {
            (false, "invoke cancelled".to_string(), peak)
        }
    }
}

fn execute_via_embedded(
    wasm_path: &Path,
    env_pairs: &[(String, String)],
    runtime_host: Option<Arc<dyn runtara_component_host::runtime_host::RuntimeHost>>,
) -> (bool, String, Option<u64>) {
    let executor = embedded_executor();
    let mut limits = runtara_component_host::WorkflowLimits::default();
    // Honor a per-run guest memory cap exactly as the production embedded runner
    // does (runtara-environment's `limits_from_env`), so a test can exercise the
    // guest OOM path without provisioning a full gigabyte of headroom.
    if let Some(max) = env_pairs
        .iter()
        .find(|(key, _)| key == "RUNTARA_INSTANCE_MEMORY_MAX_BYTES")
        .and_then(|(_, value)| value.parse::<usize>().ok())
    {
        limits.max_memory_bytes = max;
    }
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let result = runtime.block_on(async {
        let pre = executor
            .load(wasm_path)
            .await
            .expect("load composed workflow component");
        executor
            .execute(
                &pre,
                runtara_component_host::WorkflowRunSpec {
                    env: env_pairs.iter().cloned().collect(),
                    stderr: None,
                    timeout: Duration::from_secs(300),
                    cancel: None,
                    limits,
                    runtime: runtime_host,
                },
            )
            .await
    });
    eprintln!(
        "embedded run: exit={:?} memory_peak_bytes={}",
        result.exit, result.memory_peak_bytes
    );
    let peak = Some(result.memory_peak_bytes);
    match result.exit {
        runtara_component_host::WorkflowExit::Completed => (true, String::new(), peak),
        runtara_component_host::WorkflowExit::GuestError => (false, String::new(), peak),
        runtara_component_host::WorkflowExit::Failed { reason } => (false, reason, peak),
        other => (false, format!("embedded run interrupted: {other:?}"), peak),
    }
}

fn non_durable_graph_json(graph_json: &str) -> String {
    let mut graph: Value = serde_json::from_str(graph_json).expect("fixture parses as json");
    graph["durable"] = Value::Bool(false);
    serde_json::to_string(&graph).expect("graph serializes")
}

#[test]
fn direct_compile_entry_returns_native_result_shape_when_components_available() {
    let components_dir = direct_e2e_components_dir();

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
    let components_dir = direct_e2e_components_dir();

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

/// Top-level component imports of a composed `workflow.wasm`, by name.
///
/// Tracks nesting depth (every module/component start emits `Version`, every
/// end emits `End`) so only the OUTER component's import section is collected —
/// `define_components: true` embeds the stdlib/agent components as nested
/// binaries whose own imports must not be confused with the artifact's.
fn top_level_component_imports(bytes: &[u8]) -> Vec<String> {
    use wasmparser::{Parser, Payload};
    let mut depth = 0usize;
    let mut imports = Vec::new();
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.expect("parse composed component") {
            Payload::Version { .. } => depth += 1,
            Payload::End(_) => depth -= 1,
            Payload::ComponentImportSection(reader) if depth == 1 => {
                for import in reader {
                    imports.push(import.expect("component import").name.0.to_string());
                }
            }
            _ => {}
        }
    }
    imports
}

/// Spike B of docs/unify-agents-workflows-plan.md: wac-graph must compose a
/// workflow whose directly-declared `runtara:workflow-runtime/runtime` import
/// is left unsatisfied (no runtime component instantiated), surfacing it as a
/// component-level import — the same path WASI interfaces already ride. This
/// is the load-bearing assumption of the host-import migration: proven here
/// for a DIRECT workflow-logic import, not just transitive WASI ones.
#[test]
fn direct_compose_host_import_binding_surfaces_runtime_as_component_import() {
    let components_dir = direct_e2e_components_dir();
    let graph: ExecutionGraph = serde_json::from_str(SIMPLE_PASSTHROUGH).expect("fixture parses");
    let temp = tempfile::tempdir().expect("tempdir");

    let mut result = compile_direct_workflow(DirectCompilationInput {
        workflow_id: "spike-b-host-import-runtime".to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: graph,
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
    })
    .expect("direct emit succeeds");

    let agent_ids: Vec<String> = result
        .component_artifacts
        .agent_components
        .iter()
        .map(|component| component.agent_id.clone())
        .collect();

    // Control: the legacy Composed binding satisfies the runtime interface
    // internally — it must NOT appear among the composed artifact's imports.
    result.component_artifacts =
        emit_direct_component_artifacts_with_binding(&agent_ids, RuntimeBinding::Composed);
    compose_direct_workflow(&mut result, &components_dir).expect("composed-binding compose");
    let composed_bytes = fs::read(&result.wasm_path).expect("read composed artifact");
    let composed_imports = top_level_component_imports(&composed_bytes);
    assert!(
        !composed_imports
            .iter()
            .any(|name| name.starts_with("runtara:workflow-runtime/runtime")),
        "composed binding must satisfy runtime internally; imports: {composed_imports:?}"
    );
    assert!(
        composed_imports
            .iter()
            .any(|name| name.starts_with("wasi:")),
        "WASI must bubble as imports under both bindings; imports: {composed_imports:?}"
    );

    // Spike: re-emit the scaffolding under HostImport (the default) and
    // recompose. wac must type-check + encode (validate: true inside compose)
    // with the runtime interface unbound, and the interface must surface as a
    // top-level import.
    result.component_artifacts =
        emit_direct_component_artifacts_with_binding(&agent_ids, RuntimeBinding::HostImport);
    compose_direct_workflow(&mut result, &components_dir).expect("host-import-binding compose");

    let host_import_bytes = fs::read(&result.wasm_path).expect("read host-import artifact");
    let host_imports = top_level_component_imports(&host_import_bytes);
    assert!(
        host_imports
            .iter()
            .any(|name| name == "runtara:workflow-runtime/runtime@0.1.0"),
        "host-import binding must surface the runtime interface; imports: {host_imports:?}"
    );
    assert!(
        host_imports.iter().any(|name| name.starts_with("wasi:")),
        "WASI imports must survive the binding change; imports: {host_imports:?}"
    );
}

/// In-memory RuntimeHost recording the lifecycle calls a HostImport-composed
/// artifact makes. Input arrives from memory; output/error are captured from
/// the return channel — no HTTP anywhere.
struct RecordingRuntimeHost {
    input: Vec<u8>,
    completed: Mutex<Option<Vec<u8>>>,
    failed: Mutex<Option<Vec<u8>>>,
}

impl RecordingRuntimeHost {
    fn new(input: &[u8]) -> Self {
        Self {
            input: input.to_vec(),
            completed: Mutex::new(None),
            failed: Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl runtara_component_host::runtime_host::RuntimeHost for RecordingRuntimeHost {
    async fn load_input(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(Some(self.input.clone()))
    }
    fn instance_id(&self) -> Result<String, String> {
        Ok("host-import-e2e".to_string())
    }
    async fn complete(&self, output: Vec<u8>) -> Result<(), String> {
        *self.completed.lock().unwrap() = Some(output);
        Ok(())
    }
    async fn fail(&self, error: Vec<u8>) -> Result<(), String> {
        *self.failed.lock().unwrap() = Some(error);
        Ok(())
    }
    async fn custom_event(&self, _kind: String, _payload: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
    fn debug_mode_enabled(&self) -> Result<bool, String> {
        Ok(false)
    }
    async fn breakpoint_pause(&self) -> Result<(), String> {
        Ok(())
    }
    async fn heartbeat(&self) -> Result<(), String> {
        Ok(())
    }
    async fn is_cancelled(&self) -> Result<bool, String> {
        Ok(false)
    }
    async fn check_signals(&self) -> Result<bool, String> {
        Ok(false)
    }
    async fn poll_custom_signal(&self, _checkpoint_id: String) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
    async fn get_checkpoint(&self, _checkpoint_id: String) -> Result<Option<Vec<u8>>, String> {
        Ok(None)
    }
    async fn checkpoint(
        &self,
        _checkpoint_id: String,
        _state: Vec<u8>,
    ) -> Result<runtara_component_host::runtime_host::RuntimeCheckpointResult, String> {
        Ok(
            runtara_component_host::runtime_host::RuntimeCheckpointResult {
                found: false,
                state: Vec::new(),
                pending_signal: None,
                custom_signal: None,
            },
        )
    }
    async fn handle_checkpoint_signal(&self, _signal_type: String) -> Result<bool, String> {
        Ok(false)
    }
    async fn record_retry_attempt(
        &self,
        _checkpoint_id: String,
        _attempt_number: u32,
        _error_message: Option<String>,
    ) -> Result<(), String> {
        Ok(())
    }
    async fn durable_sleep_checkpoint(
        &self,
        _checkpoint_id: String,
        _state: Vec<u8>,
        _ms: u64,
    ) -> Result<(), String> {
        Ok(())
    }
}

/// Phase-1 acceptance (docs/unify-agents-workflows-plan.md): a HostImport
/// composition executes end-to-end through the in-process executor with the
/// runtime interface satisfied by native host functions — input from memory,
/// output captured from `complete` — zero HTTP. Instantiation type-checks the
/// FULL host-bound interface (all funcs + the signal/checkpoint records)
/// against the component's import, so success here proves the marshaling
/// layer, not just the happy path.
#[test]
fn direct_wasm_execute_host_import_runtime_runs_without_http() {
    let components_dir = direct_e2e_components_dir();
    let graph: ExecutionGraph = serde_json::from_str(SIMPLE_PASSTHROUGH).expect("fixture parses");
    let temp = tempfile::tempdir().expect("tempdir");

    // This test pins the RUN-shaped (legacy-export) host-import path — the
    // invoke shape has its own suite below.
    let mut result = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "phase1-host-import-exec".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        },
        WorkflowAbi::CliRunHttp,
        false,
        false,
    )
    .expect("direct emit succeeds");
    result.component_artifacts =
        emit_direct_component_artifacts_with_binding(&[], RuntimeBinding::HostImport);
    compose_direct_workflow(&mut result, &components_dir).expect("host-import compose");

    let host = Arc::new(RecordingRuntimeHost::new(br#"{"input":"host-import"}"#));
    let executor = embedded_executor();

    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let run = runtime.block_on(async {
        let pre = executor
            .load(&result.wasm_path)
            .await
            .expect("load host-import artifact");
        executor
            .execute(
                &pre,
                runtara_component_host::WorkflowRunSpec {
                    env: HashMap::new(),
                    stderr: None,
                    timeout: Duration::from_secs(60),
                    cancel: None,
                    limits: runtara_component_host::WorkflowLimits::default(),
                    runtime: Some(host.clone()),
                },
            )
            .await
    });

    assert!(
        matches!(run.exit, runtara_component_host::WorkflowExit::Completed),
        "unexpected exit: {:?} (failed: {:?})",
        run.exit,
        host.failed
            .lock()
            .unwrap()
            .as_deref()
            .map(String::from_utf8_lossy),
    );
    let output = host
        .completed
        .lock()
        .unwrap()
        .clone()
        .expect("workflow reported completion through the host import");
    let output_json: Value = serde_json::from_slice(&output).expect("output is JSON");
    assert_eq!(output_json, serde_json::json!({ "result": "host-import" }));
    assert!(host.failed.lock().unwrap().is_none(), "no failure expected");
}

#[test]
fn direct_wasm_execute_finish_passthrough_reports_completion() {
    let components_dir = direct_e2e_components_dir();

    let output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-finish-passthrough",
        SIMPLE_PASSTHROUGH,
        br#"{"input":"direct-finish"}"#,
    );

    assert_eq!(output, serde_json::json!({ "result": "direct-finish" }));
}

/// A single-Finish workflow that binds `data.count` under an `integer` type
/// hint, with an optional `default`. Exercises `apply_type_hint`'s coercion
/// through the full compile → execute path.
fn integer_hint_graph(default: Option<Value>) -> String {
    let mut reference = serde_json::json!({
        "valueType": "reference",
        "value": "data.count",
        "type": "integer"
    });
    if let Some(default) = default {
        reference["default"] = default;
    }
    let graph = serde_json::json!({
        "name": "Integer Hint Coercion",
        "steps": {
            "finish": {
                "stepType": "Finish",
                "id": "finish",
                "inputMapping": { "count": reference }
            }
        },
        "entryPoint": "finish",
        "executionPlan": [],
        "variables": {},
        "inputSchema": {},
        "outputSchema": {}
    });
    serde_json::to_string(&graph).expect("graph serializes")
}

#[test]
fn direct_wasm_execute_integer_hint_fails_loudly_on_unparseable_value() {
    let components_dir = direct_e2e_components_dir();

    // A present, non-null value that will not parse as an integer must fail the
    // run rather than silently becoming `0` and flowing into the output.
    let graph = integer_hint_graph(None);
    let failure = run_direct_workflow_expect_failure(
        &components_dir,
        "direct-wasm-execute-integer-hint-unparseable",
        &graph,
        br#"{"count":"abc"}"#,
    );

    let message = failure.error_json.to_string();
    assert!(
        message.contains("cannot be coerced to integer"),
        "expected a loud coercion failure, got: {message}"
    );
}

#[test]
fn direct_wasm_execute_integer_hint_default_rescues_unparseable_value() {
    let components_dir = direct_e2e_components_dir();

    // The author's `default` is the explicit escape hatch: the unparseable
    // value falls back to it and the run completes.
    let graph = integer_hint_graph(Some(serde_json::json!(7)));
    let output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-integer-hint-default",
        &graph,
        br#"{"count":"abc"}"#,
    );

    assert_eq!(output, serde_json::json!({ "count": 7 }));
}

#[test]
fn direct_wasm_execute_single_agent_without_finish_returns_null() {
    let components_dir = direct_e2e_components_dir();

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
    let components_dir = direct_e2e_components_dir();

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
    let components_dir = direct_e2e_components_dir();

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
fn direct_wasm_execute_fanout_cross_branch_reference_runs_producer_first() {
    let components_dir = direct_e2e_components_dir();

    // Off-backbone fan-out (inside the Conditional's false branch) where a step
    // downstream of one branch consumes the other branch's output. Both branches
    // must run, exactly once each, with `right` ordered before its consumer
    // `after_left` — the regression that dropped the second fan-out edge in the
    // CategorizeViaUnspsc repro.
    let result = run_direct_workflow_with_events_and_tracking(
        &components_dir,
        "direct-wasm-execute-fanout-cross-branch-reference",
        FANOUT_CROSS_BRANCH_REFERENCE,
        br#"{}"#,
        true,
    );

    assert_eq!(
        result.output_json,
        serde_json::json!({ "crossed": "R", "left": "L", "right": "R" })
    );

    let ended: Vec<&str> = result
        .events
        .iter()
        .filter(|event| event.subtype == "step_debug_end")
        .filter_map(|event| event.payload_json["step_id"].as_str())
        .collect();
    for step_id in ["gate", "left", "right", "after_left", "finish"] {
        assert_eq!(
            ended
                .iter()
                .filter(|ended_id| **ended_id == step_id)
                .count(),
            1,
            "step '{step_id}' should run exactly once: {ended:?}"
        );
    }
    assert!(
        !ended.contains(&"hit"),
        "the untaken Conditional branch must not run: {ended:?}"
    );
    let right_position = ended.iter().position(|step_id| *step_id == "right");
    let consumer_position = ended.iter().position(|step_id| *step_id == "after_left");
    assert!(
        right_position < consumer_position,
        "producer 'right' must run before its consumer 'after_left': {ended:?}"
    );
}

#[test]
fn direct_wasm_execute_finish_passthrough_track_events_emits_step_debug_events() {
    let components_dir = direct_e2e_components_dir();

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
fn direct_wasm_execute_agent_input_mapping_failure_records_step_error() {
    // Diagnostic-gap regression: an Agent whose input mapping fails to resolve
    // (here a template "undefined value" error) with NO onError handler used to
    // abort with only an execution-level error — its per-step record showed
    // durationMs: null, error: null. The emitter now attributes the failure to
    // the step: a step_debug_start plus an error-bearing step_debug_end, so the
    // step summary pairs them into a failed record carrying the actual error.
    let components_dir = direct_e2e_components_dir();

    let graph = r##"{
      "entryPoint": "echo",
      "executionPlan": [{"fromStep":"echo","toStep":"finish"}],
      "steps": {
        "echo": {"id":"echo","stepType":"Agent","name":"Echo",
          "agentId":"utils","capabilityId":"return-input","inputMapping":{
            "value": {"valueType":"template","value":"{{ data.missing.deep }}"}
          }},
        "finish": {"id":"finish","stepType":"Finish","inputMapping":{
          "ok": {"valueType":"immediate","value":true}
        }}
      }
    }"##;

    let captured = run_direct_workflow_capture(
        &components_dir,
        "direct-wasm-execute-agent-input-mapping-failure",
        graph,
        br#"{}"#,
        true, // track_events
    );

    assert!(
        !captured.status_success,
        "an unhandled input-mapping failure must fail the instance.\n--- stderr ---\n{}",
        captured.stderr
    );

    let start = captured
        .events
        .iter()
        .find(|e| e.subtype == "step_debug_start" && e.payload_json["step_id"] == "echo")
        .expect("the failed step must emit a step_debug_start (pre-fix: none was emitted)");
    assert_eq!(start.payload_json["step_type"], "Agent");

    let end = captured
        .events
        .iter()
        .find(|e| e.subtype == "step_debug_end" && e.payload_json["step_id"] == "echo")
        .expect("the failed step must emit an error step_debug_end (pre-fix: none was emitted)");
    assert_eq!(
        end.payload_json["outputs"]["_error"], true,
        "the step end must carry the error flag so the summary marks it failed"
    );
    let err_text = end.payload_json["outputs"]["error"]
        .as_str()
        .unwrap_or_default();
    assert!(
        err_text.contains("undefined value") || err_text.contains("Template render error"),
        "the step record must carry the actual input-resolution error, got: {err_text:?}"
    );
    assert!(
        end.payload_json["duration_ms"].as_i64().is_some(),
        "the failed step must carry a non-null duration"
    );
}

#[test]
fn direct_wasm_execute_finish_input_mapping_failure_records_step_error() {
    // Full-coverage companion to the Agent case: a non-Agent step (Finish) whose
    // input mapping fails to resolve with no onError handler must also attribute
    // the error to itself. Finish fires its step_debug_start before resolving, so
    // this exercises the generic emit_retptr_error_or_step_fail primitive (the
    // error step_debug_end pairs with the already-fired start).
    let components_dir = direct_e2e_components_dir();

    let graph = r##"{
      "entryPoint": "finish",
      "executionPlan": [],
      "steps": {
        "finish": {"id":"finish","stepType":"Finish","inputMapping":{
          "out": {"valueType":"template","value":"{{ data.missing.deep }}"}
        }}
      },
      "variables": {},
      "inputSchema": {},
      "outputSchema": {}
    }"##;

    let captured = run_direct_workflow_capture(
        &components_dir,
        "direct-wasm-execute-finish-input-mapping-failure",
        graph,
        br#"{}"#,
        true, // track_events
    );

    assert!(
        !captured.status_success,
        "an unhandled Finish input-mapping failure must fail the instance.\n--- stderr ---\n{}",
        captured.stderr
    );

    let start = captured
        .events
        .iter()
        .find(|e| e.subtype == "step_debug_start" && e.payload_json["step_id"] == "finish")
        .expect("the failed Finish must emit a step_debug_start");
    assert_eq!(start.payload_json["step_type"], "Finish");

    let end = captured
        .events
        .iter()
        .find(|e| e.subtype == "step_debug_end" && e.payload_json["step_id"] == "finish")
        .expect("the failed Finish must emit an error step_debug_end (pre-fix: none was emitted)");
    assert_eq!(end.payload_json["outputs"]["_error"], true);
    let err_text = end.payload_json["outputs"]["error"]
        .as_str()
        .unwrap_or_default();
    assert!(
        err_text.contains("undefined value") || err_text.contains("Template render error"),
        "the Finish step record must carry the input-resolution error, got: {err_text:?}"
    );
    assert!(end.payload_json["duration_ms"].as_i64().is_some());
}

#[test]
fn direct_wasm_execute_delay_duration_failure_records_step_error() {
    // Delay/Log path: these used return_if_retptr_error, which returned without
    // runtime.fail — an unresolvable config silently exited ("crashed", no
    // reason). A Delay with an unresolvable durationMs must now fail with the
    // error AND attribute it to the step (Delay emits a start before resolving).
    let components_dir = direct_e2e_components_dir();

    let graph = r##"{
      "entryPoint": "wait",
      "executionPlan": [{"fromStep":"wait","toStep":"finish"}],
      "steps": {
        "wait": {"id":"wait","stepType":"Delay","name":"Wait",
          "durationMs": {"valueType":"template","value":"{{ data.missing.deep }}"}},
        "finish": {"id":"finish","stepType":"Finish","inputMapping":{
          "ok": {"valueType":"immediate","value":true}
        }}
      },
      "variables": {},
      "inputSchema": {},
      "outputSchema": {}
    }"##;

    let captured = run_direct_workflow_capture(
        &components_dir,
        "direct-wasm-execute-delay-duration-failure",
        graph,
        br#"{}"#,
        true,
    );

    assert!(
        !captured.status_success,
        "an unresolvable Delay duration must fail the instance (not silently exit).\n--- stderr ---\n{}",
        captured.stderr
    );
    let end = captured
        .events
        .iter()
        .find(|e| e.subtype == "step_debug_end" && e.payload_json["step_id"] == "wait")
        .expect("the failed Delay must emit an error step_debug_end");
    assert_eq!(end.payload_json["outputs"]["_error"], true);
    assert!(
        captured
            .events
            .iter()
            .any(|e| e.subtype == "step_debug_start" && e.payload_json["step_id"] == "wait"),
        "the failed Delay must emit a paired step_debug_start"
    );
}

#[test]
fn direct_wasm_execute_log_payload_failure_records_step_error() {
    // A Log emits no step-debug events normally, but an unresolvable log payload
    // (broken context template) must fail with the error and be attributed: the
    // failure path emits a start + error pair so the failed Log is visible.
    let components_dir = direct_e2e_components_dir();

    let graph = r##"{
      "entryPoint": "logit",
      "executionPlan": [{"fromStep":"logit","toStep":"finish"}],
      "steps": {
        "logit": {"id":"logit","stepType":"Log","name":"Log It","level":"info","message":"hello",
          "context": {"x": {"valueType":"template","value":"{{ data.missing.deep }}"}}},
        "finish": {"id":"finish","stepType":"Finish","inputMapping":{
          "ok": {"valueType":"immediate","value":true}
        }}
      },
      "variables": {},
      "inputSchema": {},
      "outputSchema": {}
    }"##;

    let captured = run_direct_workflow_capture(
        &components_dir,
        "direct-wasm-execute-log-payload-failure",
        graph,
        br#"{}"#,
        true,
    );

    assert!(
        !captured.status_success,
        "an unresolvable Log payload must fail the instance (not silently exit).\n--- stderr ---\n{}",
        captured.stderr
    );
    let start = captured
        .events
        .iter()
        .find(|e| e.subtype == "step_debug_start" && e.payload_json["step_id"] == "logit")
        .expect("the failed Log must emit a step_debug_start on the failure path");
    assert_eq!(start.payload_json["step_type"], "Log");
    let end = captured
        .events
        .iter()
        .find(|e| e.subtype == "step_debug_end" && e.payload_json["step_id"] == "logit")
        .expect("the failed Log must emit an error step_debug_end");
    assert_eq!(end.payload_json["outputs"]["_error"], true);
}

#[test]
fn direct_wasm_execute_conditional_finish_branches_report_completion() {
    let components_dir = direct_e2e_components_dir();

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
fn direct_wasm_execute_conditional_branches_correctly_with_track_events() {
    // Regression: with track-events on, the Conditional's step-debug-end event
    // reuses the shared retptr scratch and overwrote the evaluated condition bool
    // (at offset 4), so the runtime always followed the `true` edge regardless of
    // the result. The earlier branch tests ran with track_events=false, so they
    // never exercised this. Both branches must route by the actual result.
    let components_dir = direct_e2e_components_dir();

    let true_result = run_direct_workflow_with_events_and_tracking(
        &components_dir,
        "direct-wasm-execute-conditional-true-track-events",
        CONDITIONAL_WORKFLOW,
        br#"{"flag":true}"#,
        true,
    );
    assert_eq!(
        true_result.output_json,
        serde_json::json!({ "result": "yes" })
    );

    let false_result = run_direct_workflow_with_events_and_tracking(
        &components_dir,
        "direct-wasm-execute-conditional-false-track-events",
        CONDITIONAL_WORKFLOW,
        br#"{"flag":false}"#,
        true,
    );
    assert_eq!(
        false_result.output_json,
        serde_json::json!({ "result": "no" })
    );
}

#[test]
fn direct_wasm_execute_nested_conditional_branches_report_completion() {
    let components_dir = direct_e2e_components_dir();

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
    let components_dir = direct_e2e_components_dir();

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
    let components_dir = direct_e2e_components_dir();

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
    let components_dir = direct_e2e_components_dir();

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
fn direct_wasm_execute_query_only_condition_operator_fails_loudly() {
    let components_dir = direct_e2e_components_dir();

    // GAP-01: MATCH (like SIMILARITY_GTE / COSINE_DISTANCE_LTE /
    // L2_DISTANCE_LTE) is an object-model query operator with no workflow
    // evaluator. Validation rejects new workflows up front (E027); this
    // compiles the graph directly (bypassing validation, as any workflow
    // registered before E027 existed would have) and proves the runtime now
    // fails loudly instead of silently evaluating the condition to false and
    // taking the false branch.
    let result = run_direct_workflow_expect_failure(
        &components_dir,
        "direct-wasm-execute-query-only-operator",
        CONDITIONAL_QUERY_ONLY_OPERATOR,
        br#"{"text":"haystack with needle"}"#,
    );

    // Unhandled stdlib errors surface through `runtime.fail` as a bare message
    // string (not an Error-step envelope object).
    let message = result.error_json.as_str().unwrap_or_default();
    assert!(
        message.contains("MATCH") && message.contains("object-model"),
        "expected loud query-only-operator failure, got: {}",
        result.error_json
    );
}

fn single_shot_ai_agent_graph_json(retry_config: &str) -> String {
    format!(
        r##"{{
      "entryPoint": "ai",
      "executionPlan": [
        {{"fromStep":"ai","toStep":"finish","label":"next"}}
      ],
      "steps": {{
        "ai": {{"id":"ai","stepType":"AiAgent","connectionId":"conn-1","config":{{
          "systemPrompt":{{"valueType":"immediate","value":"You are a test stub caller"}},
          "userPrompt":{{"valueType":"immediate","value":"Say hello"}},
          "provider":"openai",
          "model":"gpt-4o"{retry_config}}}}},
        "finish": {{"id":"finish","stepType":"Finish","inputMapping":{{
          "answer": {{"valueType":"reference","value":"steps.ai.outputs.response"}}
        }}}}
      }}
    }}"##
    )
}

fn llm_ok(content: &str) -> Value {
    serde_json::json!({
        "status": 200,
        "headers": {},
        "body": {
            "choices": [{"message": {"content": content}}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        }
    })
}

fn llm_http_500() -> Value {
    serde_json::json!({
        "status": 500,
        "headers": {},
        "body": {"error": {"message": "stubbed provider outage"}}
    })
}

#[test]
fn direct_wasm_execute_ai_agent_single_shot_completes_against_stub() {
    let components_dir = direct_e2e_components_dir();

    // Baseline for the hermetic LLM stub: a single-shot AiAgent drives one
    // chat-completion through the proxy and finishes with the stubbed text.
    let result = run_direct_workflow_with_llm_script(
        &components_dir,
        "ai-single-shot-stub",
        &single_shot_ai_agent_graph_json(""),
        br#"{}"#,
        vec![llm_ok("hello from stub")],
    );

    assert!(result.status_success, "stderr: {}", result.stderr);
    let output = result.output_json.expect("workflow completes");
    assert_eq!(
        output.get("answer").and_then(Value::as_str),
        Some("hello from stub"),
        "{output}"
    );
    assert_eq!(result.llm_requests.len(), 1, "exactly one model call");
    // The proxy envelope carries the OpenAI-shaped request.
    let request = &result.llm_requests[0];
    assert_eq!(
        request.get("url").and_then(Value::as_str),
        Some("/v1/chat/completions"),
        "{request}"
    );
    assert_eq!(
        request.get("ai_provider").and_then(Value::as_str),
        Some("openai"),
        "{request}"
    );
}

/// End-to-end enforcement proof: the per-attempt LLM timeout reaches the proxy
/// envelope (`timeout_ms`). With `turnTimeout` set it carries the configured
/// value; unset, it defaults to DEFAULT_STEP_TIMEOUT_MS (180000) rather than the
/// old 30s no-timeout proxy floor. This is the core "30s floor is gone" check.
#[test]
fn direct_wasm_execute_ai_agent_turn_timeout_reaches_proxy() {
    let components_dir = direct_e2e_components_dir();

    // Configured turnTimeout passes through to the proxy envelope verbatim.
    let configured = run_direct_workflow_with_llm_script(
        &components_dir,
        "ai-turn-timeout-configured",
        &single_shot_ai_agent_graph_json(",\"turnTimeout\":4321"),
        br#"{}"#,
        vec![llm_ok("hi")],
    );
    assert!(configured.status_success, "stderr: {}", configured.stderr);
    assert_eq!(configured.llm_requests.len(), 1, "exactly one model call");
    assert_eq!(
        configured.llm_requests[0]
            .get("timeout_ms")
            .and_then(Value::as_u64),
        Some(4321),
        "configured turnTimeout must reach the proxy envelope: {}",
        configured.llm_requests[0]
    );

    // Unset: the ai-tools chat capability defaults timeout_ms to
    // DEFAULT_STEP_TIMEOUT_MS, so the model call is bounded at 180s — proving
    // the prior 30s floor (timeout_ms: null -> proxy unwrap_or(30_000)) is gone.
    let defaulted = run_direct_workflow_with_llm_script(
        &components_dir,
        "ai-turn-timeout-default",
        &single_shot_ai_agent_graph_json(""),
        br#"{}"#,
        vec![llm_ok("hi")],
    );
    assert!(defaulted.status_success, "stderr: {}", defaulted.stderr);
    assert_eq!(
        defaulted.llm_requests[0]
            .get("timeout_ms")
            .and_then(Value::as_u64),
        Some(runtara_dsl::DEFAULT_STEP_TIMEOUT_MS),
        "unset turnTimeout must default to DEFAULT_STEP_TIMEOUT_MS, not the 30s floor: {}",
        defaulted.llm_requests[0]
    );
}

#[test]
fn direct_wasm_execute_ai_agent_single_shot_retries_transient_provider_errors() {
    let components_dir = direct_e2e_components_dir();

    // GAP-06: config.maxRetries drives the existing agent retry machinery for
    // the chat-completion invoke. Two stubbed HTTP 500s (transient) are
    // retried; the third call succeeds.
    let result = run_direct_workflow_with_llm_script(
        &components_dir,
        "ai-single-shot-retries",
        &single_shot_ai_agent_graph_json(r#","maxRetries":3,"retryDelay":10"#),
        br#"{}"#,
        vec![llm_http_500(), llm_http_500(), llm_ok("recovered")],
    );

    assert!(
        result.status_success,
        "retried workflow should complete; stderr: {}",
        result.stderr
    );
    let output = result
        .output_json
        .expect("workflow completes after retries");
    assert_eq!(
        output.get("answer").and_then(Value::as_str),
        Some("recovered"),
        "{output}"
    );
    assert_eq!(
        result.llm_requests.len(),
        3,
        "two failed attempts + one success"
    );
}

#[test]
fn direct_wasm_execute_durable_agent_retry_replays_attempts_across_resume() {
    // Bug fix: a durable agent step drained/restarted mid-retry must NOT
    // re-invoke attempts that already ran. Each FAILED attempt is checkpointed
    // under `{cache_key}::attempt::{N}`; on replay-from-start a per-attempt hit
    // short-circuits the invoke. "Resume" is simulated the same way as the
    // tool-loop replay test: replay against a preloaded /checkpoint store keyed by
    // the same instance_id, so the per-attempt keys match and are served back.
    let components_dir = direct_e2e_components_dir();
    let graph = single_shot_ai_agent_graph_json(r#","maxRetries":2,"retryDelay":10"#);

    // RUN 1 (original process): two transient 500s then success. Attempts 1 and 2
    // fail retryably (each persisted under `::attempt::N`); attempt 3 succeeds.
    // The `answer == "recovered"` assertion also guards the success path — if the
    // MISS path wrongly ran the error-info builder on a successful invoke it would
    // read the error struct off an ok retptr and corrupt the output.
    let run1 = run_direct_workflow_with_llm_script(
        &components_dir,
        "durable-retry-resume",
        &graph,
        br#"{}"#,
        vec![llm_http_500(), llm_http_500(), llm_ok("recovered")],
    );
    assert!(
        run1.status_success,
        "run 1 completes after retries; stderr: {}",
        run1.stderr
    );
    assert_eq!(
        run1.output_json
            .as_ref()
            .and_then(|o| o.get("answer"))
            .and_then(Value::as_str),
        Some("recovered"),
        "successful attempt output must be intact: {:?}",
        run1.output_json
    );
    assert_eq!(
        run1.llm_requests.len(),
        3,
        "att1(500) + att2(500) + att3(ok)"
    );

    // Tripwire: the two FAILED attempts are durably checkpointed. On UNFIXED code
    // no `::attempt::` checkpoints are written, so this harvest is empty. The
    // successful attempt 3 is NOT stored here (the outer step-success checkpoint
    // covers success) — only failures, keeping the extra write cost to retries.
    let attempt_checkpoints: Vec<(String, Vec<u8>)> = run1
        .checkpoints
        .iter()
        .filter(|c| c.checkpoint_id.contains("::attempt::") && !c.state.is_empty())
        .map(|c| (c.checkpoint_id.clone(), c.state.clone()))
        .collect();
    assert_eq!(
        attempt_checkpoints.len(),
        2,
        "both failed attempts must be persisted (empty on unfixed code): {:?}",
        run1.checkpoints
            .iter()
            .map(|c| &c.checkpoint_id)
            .collect::<Vec<_>>()
    );

    // RUN 2a (resume after a drain following attempt 2 — the frontier fails):
    // preload the two failed-attempt envelopes and give the resume NO live model
    // responses. A correct fix replays attempts 1 and 2 from checkpoint (zero
    // invokes) and fires ONLY the un-attempted frontier (attempt 3), which
    // exhausts the empty script and — being the last attempt (maxRetries:2) —
    // fails the workflow. On unfixed code (or a broken hit-skip) attempts 1..3 all
    // re-invoke, so this count is 3, not 1 — the direct no-re-invoke assertion.
    let resume_fail = run_direct_workflow_capture_with_preloaded_checkpoints(
        &components_dir,
        "durable-retry-resume",
        &graph,
        br#"{}"#,
        false,
        attempt_checkpoints.clone(),
        vec![],
    );
    assert!(
        !resume_fail.status_success,
        "the frontier attempt exhausts the empty script and is terminal"
    );
    assert_eq!(
        resume_fail.llm_requests.len(),
        1,
        "attempts 1-2 are replayed from checkpoint, not re-invoked; only the frontier fires"
    );

    // RUN 2b (resume after the same drain — the frontier succeeds): identical
    // preloaded state, one live success. Attempts 1 and 2 are replayed (no
    // invoke); attempt 3 succeeds on its first and only live call.
    let resume_ok = run_direct_workflow_capture_with_preloaded_checkpoints(
        &components_dir,
        "durable-retry-resume",
        &graph,
        br#"{}"#,
        false,
        attempt_checkpoints,
        vec![llm_ok("resumed")],
    );
    assert!(
        resume_ok.status_success,
        "resume completes on the frontier attempt; stderr: {}",
        resume_ok.stderr
    );
    assert_eq!(
        resume_ok
            .output_json
            .as_ref()
            .and_then(|o| o.get("answer"))
            .and_then(Value::as_str),
        Some("resumed"),
        "{:?}",
        resume_ok.output_json
    );
    assert_eq!(
        resume_ok.llm_requests.len(),
        1,
        "attempts 1-2 replayed from checkpoint; only the frontier attempt 3 invokes"
    );
}

/// A Split over its input list, each item running one durable AiAgent step with
/// `maxRetries:2`. The per-item agent's cache key folds the iteration index, so
/// its per-attempt checkpoints are `{...::[i]}::attempt::{N}` — distinct per item.
fn split_durable_agent_graph_json() -> String {
    let graph = serde_json::json!({
        "steps": {
            "split": {
                "stepType": "Split",
                "id": "split",
                "config": {
                    "value": { "valueType": "reference", "value": "data.items" },
                    "sequential": true
                },
                "subgraph": {
                    "name": "Item",
                    "entryPoint": "ai",
                    "steps": {
                        "ai": {"id":"ai","stepType":"AiAgent","connectionId":"conn-1","config":{
                            "systemPrompt":{"valueType":"immediate","value":"You are a test stub caller"},
                            "userPrompt":{"valueType":"immediate","value":"Say hello"},
                            "provider":"openai","model":"gpt-4o",
                            "maxRetries":2,"retryDelay":10
                        }},
                        "itemfinish": {"id":"itemfinish","stepType":"Finish","inputMapping":{
                            "answer": {"valueType":"reference","value":"steps.ai.outputs.response"}
                        }}
                    },
                    "executionPlan": [ {"fromStep":"ai","toStep":"itemfinish","label":"next"} ]
                }
            },
            "finish": {"id":"finish","stepType":"Finish","inputMapping":{
                "results": {"valueType":"reference","value":"steps.split.outputs"}
            }}
        },
        "entryPoint": "split",
        "executionPlan": [ {"fromStep":"split","toStep":"finish"} ],
        "variables": {},
        "inputSchema": { "items": { "type": "array" } },
        "outputSchema": {}
    });
    serde_json::to_string(&graph).expect("graph serializes")
}

#[test]
fn direct_wasm_execute_durable_agent_retry_per_iteration_isolation_across_resume() {
    // Bug-fix hardening: a durable agent retried inside a Split loop must key its
    // per-attempt checkpoints by iteration, so one iteration's stored failures can
    // never short-circuit another iteration's invoke. If `::attempt::{N}` did NOT
    // fold the loop index, item 1's attempt 1 would hit item 0's envelope.
    let components_dir = direct_e2e_components_dir();
    let graph = split_durable_agent_graph_json();
    let input = br#"{"data":{"items":[0,1]},"variables":{}}"#;

    // RUN 1: both items run sequentially; each agent fails twice then succeeds.
    // The shared FIFO model script is consumed item 0 first, then item 1. If the
    // per-attempt keys collided across iterations, item 1's early attempts would
    // hit item 0's checkpoints and fire fewer calls — so `llm_requests == 6`
    // itself proves the two iterations invoked independently.
    let run1 = run_direct_workflow_with_llm_script(
        &components_dir,
        "split-durable-retry-resume",
        &graph,
        input,
        vec![
            llm_http_500(),
            llm_http_500(),
            llm_ok("item0"),
            llm_http_500(),
            llm_http_500(),
            llm_ok("item1"),
        ],
    );
    assert!(
        run1.status_success,
        "run 1 completes both items after retries; stderr: {}",
        run1.stderr
    );
    assert_eq!(
        run1.llm_requests.len(),
        6,
        "2 items x (2 failed + 1 success); a per-iteration key collision would lower this"
    );

    let attempt_checkpoints: Vec<(String, Vec<u8>)> = run1
        .checkpoints
        .iter()
        .filter(|c| c.checkpoint_id.contains("::attempt::") && !c.state.is_empty())
        .map(|c| (c.checkpoint_id.clone(), c.state.clone()))
        .collect();
    // Four distinct failed-attempt checkpoints: two per iteration, iteration index
    // folded into the key so item 0 and item 1 never collide.
    assert_eq!(
        attempt_checkpoints.len(),
        4,
        "two failed attempts per item, iteration-scoped: {:?}",
        run1.checkpoints
            .iter()
            .map(|c| &c.checkpoint_id)
            .collect::<Vec<_>>()
    );
    let item0_keys = attempt_checkpoints
        .iter()
        .filter(|(id, _)| id.contains("[0]"))
        .count();
    let item1_keys = attempt_checkpoints
        .iter()
        .filter(|(id, _)| id.contains("[1]"))
        .count();
    assert_eq!(
        (item0_keys, item1_keys),
        (2, 2),
        "each iteration must own two distinct per-attempt keys: {:?}",
        attempt_checkpoints
            .iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>()
    );

    // RUN 2 (resume after a drain mid-retry across the loop): preload all four
    // per-iteration envelopes and give exactly one live success per item. Each
    // iteration replays its OWN two failed attempts (zero invokes) and fires only
    // its frontier — 2 live calls total. A collision would make one item consume
    // the other's checkpoints and diverge from this count.
    let resume = run_direct_workflow_capture_with_preloaded_checkpoints(
        &components_dir,
        "split-durable-retry-resume",
        &graph,
        input,
        false,
        attempt_checkpoints,
        vec![llm_ok("item0-resumed"), llm_ok("item1-resumed")],
    );
    assert!(
        resume.status_success,
        "resume completes both items on their frontier attempts; stderr: {}",
        resume.stderr
    );
    assert_eq!(
        resume.llm_requests.len(),
        2,
        "each iteration replays its own 2 attempts and fires only its frontier"
    );
}

#[test]
fn direct_wasm_execute_ai_agent_single_shot_default_does_not_retry() {
    let components_dir = direct_e2e_components_dir();

    // Default stays 0 retries: re-billing an LLM call is opt-in. The first
    // stubbed 500 fails the workflow; the scripted success is never consumed.
    let result = run_direct_workflow_capture_with_preloaded_checkpoints(
        &components_dir,
        "ai-single-shot-no-retries",
        &single_shot_ai_agent_graph_json(""),
        br#"{}"#,
        false,
        Vec::new(),
        vec![llm_http_500(), llm_ok("never reached")],
    );

    assert!(
        !result.status_success,
        "default must fail on the first provider error"
    );
    assert_eq!(result.llm_requests.len(), 1, "no retry call may happen");
    let error = result.error_json.expect("failure is posted");
    let message = error.to_string();
    assert!(
        message.contains("500") || message.contains("provider outage"),
        "unexpected failure payload: {message}"
    );
}

fn ai_agent_tool_loop_graph_json() -> String {
    r##"{
      "entryPoint": "ai",
      "executionPlan": [
        {"fromStep":"ai","toStep":"finish","label":"next"},
        {"fromStep":"ai","toStep":"echo_tool","label":"echo"}
      ],
      "steps": {
        "ai": {"id":"ai","stepType":"AiAgent","connectionId":"conn-1","breakpoint":true,"config":{
          "systemPrompt":{"valueType":"immediate","value":"You call tools"},
          "userPrompt":{"valueType":"immediate","value":"Use the echo tool"},
          "provider":"openai",
          "model":"gpt-4o"}},
        "echo_tool": {"id":"echo_tool","stepType":"Agent","name":"echo",
          "agentId":"utils","capabilityId":"return-input","inputMapping":{}},
        "finish": {"id":"finish","stepType":"Finish","inputMapping":{
          "answer": {"valueType":"reference","value":"steps.ai.outputs.response"}
        }}
      }
    }"##
    .to_string()
}

fn llm_tool_call(tool_name: &str, arguments: &str) -> Value {
    serde_json::json!({
        "status": 200,
        "headers": {},
        "body": {
            "choices": [{"message": {"tool_calls": [{
                "id": "call_1",
                "function": {"name": tool_name, "arguments": arguments}
            }]}}]
        }
    })
}

#[test]
fn direct_wasm_execute_ai_agent_loop_breakpoint_pauses_before_first_llm_call() {
    let components_dir = direct_e2e_components_dir();

    // GAP-08: with debug mode on, a breakpoint on a tool-loop AiAgent pauses
    // BEFORE any loop work - no memory load, no model call. The run exits
    // cleanly without /completed or /failed, stores the breakpoint-hit
    // checkpoint, and emits the breakpoint_hit event. The empty LLM script
    // proves zero model calls (any call would fail loudly on script
    // exhaustion).
    let result = run_direct_workflow_capture_full(
        &components_dir,
        "ai-loop-breakpoint-pause",
        &ai_agent_tool_loop_graph_json(),
        br#"{}"#,
        false,
        Vec::new(),
        Vec::new(),
        vec![("DEBUG_MODE".to_string(), "true".to_string())],
    );

    assert!(
        result.status_success,
        "breakpoint pause is a clean exit; stderr: {}",
        result.stderr
    );
    assert!(result.output_json.is_none(), "paused run must not complete");
    assert!(result.error_json.is_none(), "paused run must not fail");
    assert_eq!(result.llm_requests.len(), 0, "paused before any model call");
    assert!(
        result
            .checkpoints
            .iter()
            .any(|checkpoint| checkpoint.checkpoint_id == "breakpoint::ai"),
        "breakpoint-hit checkpoint must be stored: {:?}",
        result
            .checkpoints
            .iter()
            .map(|c| &c.checkpoint_id)
            .collect::<Vec<_>>()
    );
    assert!(
        result
            .events
            .iter()
            .any(|event| event.subtype == "breakpoint_hit"),
        "breakpoint_hit event must be emitted: {:?}",
        result.events.iter().map(|e| &e.subtype).collect::<Vec<_>>()
    );
}

#[test]
fn direct_wasm_execute_ai_agent_loop_breakpoint_resumes_with_checkpoint() {
    let components_dir = direct_e2e_components_dir();

    // Resume: the breakpoint-hit checkpoint short-circuits the pause and the
    // tool loop runs to completion - one tool-call turn against the echo
    // tool, then a completing turn.
    let result = run_direct_workflow_capture_full(
        &components_dir,
        "ai-loop-breakpoint-resume",
        &ai_agent_tool_loop_graph_json(),
        br#"{}"#,
        false,
        vec![(
            "breakpoint::ai".to_string(),
            br#""breakpoint_hit""#.to_vec(),
        )],
        vec![
            llm_tool_call("echo", r#"{"value":42}"#),
            llm_ok("loop finished"),
        ],
        vec![("DEBUG_MODE".to_string(), "true".to_string())],
    );

    assert!(
        result.status_success,
        "resumed run should complete; stderr: {}",
        result.stderr
    );
    let output = result.output_json.expect("resumed run completes");
    assert_eq!(
        output.get("answer").and_then(Value::as_str),
        Some("loop finished"),
        "{output}"
    );
    assert_eq!(
        result.llm_requests.len(),
        2,
        "tool-call turn + completing turn"
    );
}

/// The tool-loop graph with a raised turn budget so a long conversation can run
/// past the default 10-turn safety bound.
fn ai_agent_tool_loop_graph_with_max(max_iterations: u32) -> String {
    ai_agent_tool_loop_graph_json().replace(
        r#""model":"gpt-4o"}}"#,
        &format!(r#""model":"gpt-4o","maxIterations":{max_iterations}}}}}"#),
    )
}

// 24 KiB echoed back into the conversation each of ~50 turns. The conversation
// (carried in the loop's STATE survivor) grows ~linearly, and each turn's input
// embeds that whole conversation, so the un-freed per-turn scratch (turn input,
// model output, tool result) accumulates O(N^2) across the run — the same
// unbounded-bump leak the Split/While reset fixed, now per turn. The per-turn
// arena reset bounds the live set to ~the final conversation plus one turn's
// working set.
const AI_LEAK_TOOL_BYTES: usize = 24 * 1024;
const AI_LEAK_TURNS: usize = 50;
const AI_LEAK_MEM_CAP_BYTES: usize = 96 * 1024 * 1024;

/// Regression for the AiAgent loop's per-turn heap reset (Stage 4): a long
/// tool-calling conversation that echoes a large payload every turn must not grow
/// guest memory per turn. With the reset the run completes with a bounded peak;
/// without it the O(N^2) per-turn scratch balloons past the cap. Asserts the
/// FIXED behavior — the loop completes all turns (no state corruption over a long
/// conversation) AND the guest peak stays well under a cap the un-reset O(N^2)
/// would exceed. Relies on the keep-alive-fixed mock server to sustain the
/// hundreds of HTTP round-trips a 50-turn loop makes.
#[test]
fn ai_agent_loop_long_conversation_stays_bounded() {
    let components_dir = direct_e2e_components_dir();

    // Each turn the model "requests" an echo tool call carrying a large blob; the
    // echo tool returns it, so both the tool-call message and its result grow the
    // conversation. The final turn answers, exiting the loop.
    let payload = "z".repeat(AI_LEAK_TOOL_BYTES);
    let args = serde_json::json!({ "blob": payload }).to_string();
    let mut script: Vec<Value> = (0..AI_LEAK_TURNS - 1)
        .map(|_| llm_tool_call("echo", &args))
        .collect();
    script.push(llm_ok("done"));

    let captured = run_direct_workflow_capture_full(
        &components_dir,
        "ai-loop-long-conversation",
        &ai_agent_tool_loop_graph_with_max(AI_LEAK_TURNS as u32 + 5),
        br#"{}"#,
        false,
        Vec::new(),
        script,
        vec![(
            "RUNTARA_INSTANCE_MEMORY_MAX_BYTES".into(),
            AI_LEAK_MEM_CAP_BYTES.to_string(),
        )],
    );

    assert!(
        captured.status_success,
        "long AiAgent conversation should complete; stderr={:?} error_json={:?}",
        captured.stderr, captured.error_json,
    );
    let peak = captured
        .memory_peak_bytes
        .expect("embedded executor reports a memory peak");
    assert!(
        peak < 48 * 1024 * 1024,
        "per-turn scratch not reclaimed: peak {peak} bytes over {} turns \
         (expected bounded to ~the final conversation's working set)",
        AI_LEAK_TURNS,
    );
}

fn ai_agent_tool_only_no_next_graph_json() -> String {
    r##"{
      "entryPoint": "ai",
      "executionPlan": [
        {"fromStep":"ai","toStep":"echo_tool","label":"echo"}
      ],
      "steps": {
        "ai": {"id":"ai","stepType":"AiAgent","connectionId":"conn-1","config":{
          "systemPrompt":{"valueType":"immediate","value":"You call tools"},
          "userPrompt":{"valueType":"immediate","value":"List all tools you have"},
          "provider":"openai",
          "model":"gpt-4o"}},
        "echo_tool": {"id":"echo_tool","stepType":"Agent","name":"echo",
          "agentId":"utils","capabilityId":"return-input","inputMapping":{}}
      }
    }"##
    .to_string()
}

#[test]
fn direct_wasm_execute_ai_agent_tool_loop_without_next_edge_runs_loop() {
    let components_dir = direct_e2e_components_dir();

    // Regression: a tool-loop AiAgent whose ONLY outgoing edge is the tool
    // edge (no "next" edge, no Finish step) ran the loop but emitted no step
    // events at all — the UI showed the step as never executed. Mirrors a
    // UI-authored workflow where the agent is terminal.
    let result = run_direct_workflow_capture_full(
        &components_dir,
        "ai-loop-no-next",
        &ai_agent_tool_only_no_next_graph_json(),
        br#"{}"#,
        true,
        Vec::new(),
        vec![
            llm_tool_call("echo", r#"{"value":42}"#),
            llm_ok("loop finished"),
        ],
        Vec::new(),
    );

    assert!(
        result.status_success,
        "run should not crash; stderr: {}",
        result.stderr
    );
    assert_eq!(
        result.llm_requests.len(),
        2,
        "tool-call turn + completing turn; events: {:?}, stderr: {}",
        result.events.iter().map(|e| &e.subtype).collect::<Vec<_>>(),
        result.stderr
    );

    let event_keys: Vec<(String, String)> = result
        .events
        .iter()
        .map(|event| {
            (
                event.subtype.clone(),
                event
                    .payload_json
                    .get("step_id")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string(),
            )
        })
        .collect();

    // The AiAgent step itself emits paired debug events.
    assert!(
        event_keys.contains(&("step_debug_start".to_string(), "ai".to_string())),
        "AI step must emit step_debug_start: {event_keys:?}"
    );
    assert!(
        event_keys.contains(&("step_debug_end".to_string(), "ai".to_string())),
        "AI step must emit step_debug_end: {event_keys:?}"
    );

    // The dispatched tool call appears as a synthetic AiAgentToolCall step,
    // matching the generated compiler's "{step}.tool.{name}.{call}" events.
    assert!(
        event_keys.contains(&("step_debug_start".to_string(), "ai.tool.echo.1".to_string())),
        "tool call must emit step_debug_start: {event_keys:?}"
    );
    let tool_end = result
        .events
        .iter()
        .find(|event| {
            event.subtype == "step_debug_end"
                && event.payload_json.get("step_id").and_then(Value::as_str)
                    == Some("ai.tool.echo.1")
        })
        .expect("tool call must emit step_debug_end");
    assert_eq!(
        tool_end.payload_json["step_type"],
        serde_json::json!("AiAgentToolCall")
    );
    assert_eq!(
        tool_end.payload_json["outputs"]["outputs"]["tool_name"],
        serde_json::json!("echo")
    );

    // The AI step's debug-end carries the legacy {response, iterations,
    // toolCalls} envelope.
    let ai_end = result
        .events
        .iter()
        .find(|event| {
            event.subtype == "step_debug_end"
                && event.payload_json.get("step_id").and_then(Value::as_str) == Some("ai")
        })
        .expect("AI step debug end");
    assert_eq!(
        ai_end.payload_json["outputs"]["outputs"]["response"],
        serde_json::json!("loop finished")
    );
    assert_eq!(
        ai_end.payload_json["outputs"]["outputs"]["toolCalls"][0]["tool_name"],
        serde_json::json!("echo")
    );
}

#[test]
fn direct_wasm_execute_ai_agent_tool_loop_with_next_edge_emits_debug_events() {
    let components_dir = direct_e2e_components_dir();

    // Control for the no-next-edge case: same tool loop with a "next" edge
    // and Finish, trackEvents on.
    let result = run_direct_workflow_capture_full(
        &components_dir,
        "ai-loop-with-next-events",
        &ai_agent_tool_loop_graph_json(),
        br#"{}"#,
        true,
        Vec::new(),
        vec![llm_ok("loop finished")],
        Vec::new(),
    );

    assert!(
        result.status_success,
        "run should not crash; stderr: {}",
        result.stderr
    );
    assert_eq!(result.llm_requests.len(), 1, "model called once");
    let event_keys: Vec<(String, String)> = result
        .events
        .iter()
        .map(|event| {
            (
                event.subtype.clone(),
                event
                    .payload_json
                    .get("step_id")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string(),
            )
        })
        .collect();
    assert!(
        event_keys.contains(&("step_debug_start".to_string(), "ai".to_string())),
        "AI step itself must emit step_debug_start: {event_keys:?}"
    );
    assert!(
        event_keys.contains(&("step_debug_end".to_string(), "ai".to_string())),
        "AI step itself must emit step_debug_end: {event_keys:?}"
    );
    // The Finish step's events still follow the AI step's.
    assert!(
        event_keys.contains(&("step_debug_start".to_string(), "finish".to_string())),
        "Finish step events present: {event_keys:?}"
    );
}

fn ai_agent_memory_graph_json() -> String {
    r##"{
      "entryPoint": "ai",
      "executionPlan": [
        {"fromStep":"ai","toStep":"finish","label":"next"},
        {"fromStep":"ai","toStep":"mem","label":"memory"}
      ],
      "steps": {
        "ai": {"id":"ai","stepType":"AiAgent","connectionId":"conn-1","config":{
          "systemPrompt":{"valueType":"immediate","value":"You chat"},
          "userPrompt":{"valueType":"immediate","value":"Say hello"},
          "provider":"openai",
          "model":"gpt-4o",
          "memory":{
            "conversationId":{"valueType":"immediate","value":"conv-42"},
            "compaction":{"maxMessages":1}
          }}},
        "mem": {"id":"mem","stepType":"Agent","name":"Memory","agentId":"object-model",
          "capabilityId":"load-memory","connectionId":"conn-1","inputMapping":{}},
        "finish": {"id":"finish","stepType":"Finish","inputMapping":{
          "answer": {"valueType":"reference","value":"steps.ai.outputs.response"}
        }}
      }
    }"##
    .to_string()
}

#[test]
fn direct_wasm_execute_ai_agent_memory_emits_debug_events() {
    let components_dir = direct_e2e_components_dir();

    // Conversation memory phases must surface as synthetic AiAgentMemory*
    // steps like the generated compiler: load before the loop, sliding-window
    // compaction (maxMessages 1 < the 2-message history, so it fires) and
    // save after. The object-model provider's HTTP calls hit the mock's
    // generic `{"success": true}` fallback — an empty stored conversation.
    let result = run_direct_workflow_capture_full(
        &components_dir,
        "ai-memory-events",
        &ai_agent_memory_graph_json(),
        br#"{}"#,
        true,
        Vec::new(),
        vec![llm_ok("hello there")],
        Vec::new(),
    );

    assert!(
        result.status_success,
        "run should complete; stderr: {}; error: {:?}; events: {:?}; llm calls: {}",
        result.stderr,
        result.error_json,
        result
            .events
            .iter()
            .map(|e| (
                e.subtype.clone(),
                e.payload_json
                    .get("step_id")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string()
            ))
            .collect::<Vec<_>>(),
        result.llm_requests.len(),
    );
    let output = result.output_json.expect("run completes");
    assert_eq!(
        output.get("answer").and_then(Value::as_str),
        Some("hello there"),
        "{output}"
    );

    let event_keys: Vec<(String, String)> = result
        .events
        .iter()
        .map(|event| {
            (
                event.subtype.clone(),
                event
                    .payload_json
                    .get("step_id")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string(),
            )
        })
        .collect();
    for step_id in ["ai.memory_load", "ai.memory.compact", "ai.memory_save"] {
        for subtype in ["step_debug_start", "step_debug_end"] {
            assert!(
                event_keys.contains(&(subtype.to_string(), step_id.to_string())),
                "missing {subtype} for {step_id}: {event_keys:?}"
            );
        }
    }

    let find_end = |step_id: &str| {
        result
            .events
            .iter()
            .find(|event| {
                event.subtype == "step_debug_end"
                    && event.payload_json.get("step_id").and_then(Value::as_str) == Some(step_id)
            })
            .unwrap_or_else(|| panic!("missing debug end for {step_id}"))
    };

    // Load: the mock has no stored conversation — empty history.
    let load_end = find_end("ai.memory_load");
    assert_eq!(
        load_end.payload_json["step_type"],
        serde_json::json!("AiAgentMemoryLoad")
    );
    assert_eq!(
        load_end.payload_json["outputs"]["message_count"],
        serde_json::json!(0)
    );

    // Compaction: the turn leaves [user, assistant]; maxMessages 1 drops one.
    let compact_end = find_end("ai.memory.compact");
    assert_eq!(
        compact_end.payload_json["outputs"]["outputs"],
        serde_json::json!({
            "strategy": "sliding_window",
            "success": true,
            "messages_before": 2,
            "messages_after": 1,
            "messages_dropped": 1
        })
    );

    // Save: the compacted single-message history is persisted.
    let save_end = find_end("ai.memory_save");
    assert_eq!(
        save_end.payload_json["outputs"]["success"],
        serde_json::json!(true)
    );
    assert_eq!(
        save_end.payload_json["outputs"]["message_count"],
        serde_json::json!(1)
    );
}

fn ai_agent_tool_loop_durable_graph_json(durable: bool) -> String {
    format!(
        r##"{{
      "entryPoint": "ai",
      "executionPlan": [
        {{"fromStep":"ai","toStep":"finish","label":"next"}},
        {{"fromStep":"ai","toStep":"echo_tool","label":"echo"}}
      ],
      "steps": {{
        "ai": {{"id":"ai","stepType":"AiAgent","connectionId":"conn-1","durable":{durable},"config":{{
          "systemPrompt":{{"valueType":"immediate","value":"You call tools"}},
          "userPrompt":{{"valueType":"immediate","value":"Use the echo tool"}},
          "provider":"openai",
          "model":"gpt-4o"}}}},
        "echo_tool": {{"id":"echo_tool","stepType":"Agent","name":"echo",
          "agentId":"utils","capabilityId":"return-input","inputMapping":{{}}}},
        "finish": {{"id":"finish","stepType":"Finish","inputMapping":{{
          "answer": {{"valueType":"reference","value":"steps.ai.outputs.response"}}
        }}}}
      }}
    }}"##
    )
}

#[test]
fn direct_wasm_execute_ai_agent_loop_replays_completed_turns_without_rebilling() {
    let components_dir = direct_e2e_components_dir();

    // GAP-04: each completed turn is checkpointed under {step}.turn.{n}.
    // Run 1 completes the tool-call turn (turn 1: LLM + echo tool dispatch)
    // and then dies on a provider error at turn 2 - a mid-loop crash.
    let crashed = run_direct_workflow_with_llm_script(
        &components_dir,
        "ai-loop-durability",
        &ai_agent_tool_loop_durable_graph_json(true),
        br#"{}"#,
        vec![llm_tool_call("echo", r#"{"value":42}"#), llm_http_500()],
    );
    assert!(
        !crashed.status_success,
        "run 1 must fail at the second turn"
    );
    assert_eq!(crashed.llm_requests.len(), 2, "turn 1 + failed turn 2");
    // The capture stream includes the empty lookup PROBES as well as the
    // saves; only non-empty states are real stored checkpoints.
    let turn_checkpoints: Vec<(String, Vec<u8>)> = crashed
        .checkpoints
        .iter()
        .filter(|checkpoint| {
            checkpoint.checkpoint_id.starts_with("ai.turn.") && !checkpoint.state.is_empty()
        })
        .map(|checkpoint| (checkpoint.checkpoint_id.clone(), checkpoint.state.clone()))
        .collect();
    assert!(
        turn_checkpoints.iter().any(|(id, _)| id == "ai.turn.1"),
        "turn 1 must be checkpointed before the crash: {:?}",
        crashed
            .checkpoints
            .iter()
            .map(|c| &c.checkpoint_id)
            .collect::<Vec<_>>()
    );

    // Run 2 (replay after the crash): the preloaded turn-1 snapshot restores
    // the conversation + tool results WITHOUT a model call or tool dispatch;
    // only the failed turn 2 runs live. Exactly ONE model call - turn 1 is
    // not re-billed.
    let resumed = run_direct_workflow_capture_with_preloaded_checkpoints(
        &components_dir,
        "ai-loop-durability",
        &ai_agent_tool_loop_durable_graph_json(true),
        br#"{}"#,
        false,
        turn_checkpoints,
        vec![llm_ok("done after resume")],
    );
    assert!(
        resumed.status_success,
        "replay must complete; error: {:?}; events: {:?}; stderr: {}",
        resumed.error_json,
        resumed
            .events
            .iter()
            .map(|event| &event.subtype)
            .collect::<Vec<_>>(),
        resumed.stderr
    );
    let output = resumed.output_json.expect("replay completes");
    assert_eq!(
        output.get("answer").and_then(Value::as_str),
        Some("done after resume"),
        "{output}"
    );
    assert_eq!(
        resumed.llm_requests.len(),
        1,
        "completed turn 1 must NOT be re-billed on replay"
    );
    // The replayed turn-2 request must carry turn 1's tool result in the
    // conversation - the restored snapshot, not a fresh conversation.
    let request_body = resumed.llm_requests[0].to_string();
    assert!(
        request_body.contains("42"),
        "replayed turn must see turn 1's tool result: {request_body}"
    );
}

#[test]
fn direct_wasm_execute_ai_agent_loop_non_durable_skips_turn_checkpoints() {
    let components_dir = direct_e2e_components_dir();

    // durable:false opts the loop out of per-turn checkpoints entirely.
    let result = run_direct_workflow_with_llm_script(
        &components_dir,
        "ai-loop-non-durable",
        &ai_agent_tool_loop_durable_graph_json(false),
        br#"{}"#,
        vec![
            llm_tool_call("echo", r#"{"value":1}"#),
            llm_ok("non-durable done"),
        ],
    );

    assert!(result.status_success, "stderr: {}", result.stderr);
    assert_eq!(result.llm_requests.len(), 2);
    assert!(
        !result
            .checkpoints
            .iter()
            .any(|checkpoint| checkpoint.checkpoint_id.starts_with("ai.turn.")),
        "non-durable loop must not write turn checkpoints: {:?}",
        result
            .checkpoints
            .iter()
            .map(|c| &c.checkpoint_id)
            .collect::<Vec<_>>()
    );
}

fn ai_agent_tool_loop_on_error_graph_json(tool_capability: &str) -> String {
    format!(
        r##"{{
      "entryPoint": "ai",
      "executionPlan": [
        {{"fromStep":"ai","toStep":"finish","label":"next"}},
        {{"fromStep":"ai","toStep":"echo_tool","label":"echo"}},
        {{"fromStep":"ai","toStep":"handler_finish","label":"onError"}}
      ],
      "steps": {{
        "ai": {{"id":"ai","stepType":"AiAgent","connectionId":"conn-1","config":{{
          "systemPrompt":{{"valueType":"immediate","value":"You call tools"}},
          "userPrompt":{{"valueType":"immediate","value":"Use the echo tool"}},
          "provider":"openai",
          "model":"gpt-4o"}}}},
        "echo_tool": {{"id":"echo_tool","stepType":"Agent","name":"echo",
          "agentId":"utils","capabilityId":"{tool_capability}","inputMapping":{{}}}},
        "handler_finish": {{"id":"handler_finish","stepType":"Finish","inputMapping":{{
          "handled": {{"valueType":"immediate","value":true}},
          "code": {{"valueType":"reference","value":"steps.__error.code"}}
        }}}},
        "finish": {{"id":"finish","stepType":"Finish","inputMapping":{{
          "answer": {{"valueType":"reference","value":"steps.ai.outputs.response"}}
        }}}}
      }}
    }}"##
    )
}

#[test]
fn direct_wasm_execute_ai_agent_loop_provider_error_routes_to_on_error() {
    let components_dir = direct_e2e_components_dir();

    // GAP-05: a chat-turn (provider) failure inside the tool loop routes to
    // the step's onError handler instead of failing the workflow. The handler
    // Finish reads steps.__error, so the workflow COMPLETES with the handler
    // output.
    let result = run_direct_workflow_with_llm_script(
        &components_dir,
        "ai-loop-on-error-provider",
        &ai_agent_tool_loop_on_error_graph_json("return-input"),
        br#"{}"#,
        vec![llm_http_500()],
    );

    assert!(
        result.status_success,
        "handler completion is a clean exit; error: {:?}; stderr: {}",
        result.error_json, result.stderr
    );
    let output = result.output_json.expect("handler Finish completes");
    assert_eq!(
        output.get("handled").and_then(Value::as_bool),
        Some(true),
        "{output}"
    );
    assert_eq!(
        output.get("code").and_then(Value::as_str),
        Some("AI_TURN_COMPLETION_FAILED"),
        "handler must see the chat-turn error envelope: {output}"
    );
    assert_eq!(result.llm_requests.len(), 1);
}

#[test]
fn direct_wasm_execute_ai_agent_loop_tool_error_feeds_back_not_on_error() {
    let components_dir = direct_e2e_components_dir();

    // Guard the unchanged semantics: an individual TOOL failure (unknown
    // capability here) is fed back to the model as the tool result; the loop
    // continues and the NORMAL finish runs - onError is not taken.
    let result = run_direct_workflow_with_llm_script(
        &components_dir,
        "ai-loop-tool-error-feedback",
        &ai_agent_tool_loop_on_error_graph_json("definitely-not-a-capability"),
        br#"{}"#,
        vec![
            llm_tool_call("echo", r#"{"value":1}"#),
            llm_ok("recovered from tool error"),
        ],
    );

    assert!(
        result.status_success,
        "error: {:?}; stderr: {}",
        result.error_json, result.stderr
    );
    let output = result.output_json.expect("normal finish completes");
    assert_eq!(
        output.get("answer").and_then(Value::as_str),
        Some("recovered from tool error"),
        "tool failures must not route to onError: {output}"
    );
    assert_eq!(result.llm_requests.len(), 2);
    // The second model call must carry the tool error envelope as the result.
    let second_request = result.llm_requests[1].to_string();
    assert!(
        second_request.to_lowercase().contains("error"),
        "tool error envelope must feed back to the model: {second_request}"
    );
}

#[test]
fn direct_wasm_execute_agent_source_edge_conditions_route_on_agent_output() {
    let components_dir = direct_e2e_components_dir();

    // GAP-13: conditioned normal-flow edges from an AGENT source route on the
    // agent's own output (steps.echo.outputs.*), with priority ordering and
    // the default fallback — and coexist with an onError edge on the same
    // step (success path must take the EdgeRoute, not the handler).
    for (input, expected_path) in [
        // tier=vip outranks status=active (priority 10 > 5)
        (r#"{"status":"active","tier":"vip"}"#, "vip"),
        (r#"{"status":"active"}"#, "active"),
        (r#"{"status":"dormant"}"#, "default"),
    ] {
        let output = run_direct_workflow(
            &components_dir,
            "agent-edge-condition",
            AGENT_EDGE_CONDITION,
            input.as_bytes(),
        );
        assert_eq!(
            output.get("path").and_then(Value::as_str),
            Some(expected_path),
            "input {input} routed wrong: {output}"
        );
    }
}

#[test]
fn direct_wasm_execute_wait_timeout_routes_to_on_error() {
    let components_dir = direct_e2e_components_dir();

    // GAP-14: the 1ms wait deadline expires (the mock runtime never delivers
    // a signal) and the WAIT_TIMEOUT envelope routes to the onError handler,
    // which completes the workflow reading steps.__error.*.
    let output = run_direct_workflow(
        &components_dir,
        "wait-timeout-on-error",
        WAIT_TIMEOUT_ON_ERROR,
        br#"{}"#,
    );

    assert_eq!(
        output.get("handled").and_then(Value::as_bool),
        Some(true),
        "{output}"
    );
    assert_eq!(
        output.get("code").and_then(Value::as_str),
        Some("WAIT_TIMEOUT"),
        "{output}"
    );
    assert_eq!(
        output.get("category").and_then(Value::as_str),
        Some("timeout"),
        "{output}"
    );
}

#[test]
fn direct_wasm_compile_single_shot_ai_agent_gate_checks_on_error_handler() {
    let components_dir = direct_e2e_components_dir();

    // GAP-07: a single-shot AiAgent's onError handler is lowered live, so the
    // support gate must shape-check it. A handler whose Conditional lacks a
    // `false` branch is rejected AT THE GATE with a per-feature report (it
    // previously slipped through and died at plan build); the same workflow
    // with a well-formed handler compiles and composes to a runnable wasm.
    let malformed = r##"{
      "entryPoint": "ai",
      "executionPlan": [
        {"fromStep":"ai","toStep":"finish","label":"next"},
        {"fromStep":"ai","toStep":"handler_check","label":"onError"},
        {"fromStep":"handler_check","toStep":"handler_finish","label":"true"}
      ],
      "steps": {
        "ai": {"id":"ai","stepType":"AiAgent","connectionId":"conn-1","config":{
          "systemPrompt":{"valueType":"immediate","value":"sys"},
          "userPrompt":{"valueType":"immediate","value":"go"},
          "provider":"openai"}},
        "handler_check": {"id":"handler_check","stepType":"Conditional","condition":{
          "type":"operation","op":"EQ","arguments":[
            {"valueType":"immediate","value":1},
            {"valueType":"immediate","value":1}]}},
        "handler_finish": {"id":"handler_finish","stepType":"Finish"},
        "finish": {"id":"finish","stepType":"Finish"}
      }
    }"##;
    let graph: ExecutionGraph = serde_json::from_str(malformed).expect("fixture parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let error = compile_direct_workflow_composed(
        DirectCompilationInput {
            workflow_id: "ai-gate-malformed-handler".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        },
        &components_dir,
    )
    .expect_err("malformed single-shot handler must fail at the gate");
    let message = error.to_string();
    assert!(
        message.contains("does not support this graph"),
        "expected a gate Unsupported error, got: {message}"
    );

    let well_formed = r##"{
      "entryPoint": "ai",
      "executionPlan": [
        {"fromStep":"ai","toStep":"finish","label":"next"},
        {"fromStep":"ai","toStep":"handler_finish","label":"onError"}
      ],
      "steps": {
        "ai": {"id":"ai","stepType":"AiAgent","connectionId":"conn-1","config":{
          "systemPrompt":{"valueType":"immediate","value":"sys"},
          "userPrompt":{"valueType":"immediate","value":"go"},
          "provider":"openai"}},
        "handler_finish": {"id":"handler_finish","stepType":"Finish"},
        "finish": {"id":"finish","stepType":"Finish"}
      }
    }"##;
    let graph: ExecutionGraph = serde_json::from_str(well_formed).expect("fixture parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let compiled = compile_direct_workflow_composed(
        DirectCompilationInput {
            workflow_id: "ai-gate-well-formed-handler".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        },
        &components_dir,
    )
    .expect("well-formed single-shot handler must compile and compose");
    assert!(
        fs::metadata(&compiled.wasm_path)
            .expect("composed wasm exists")
            .len()
            > 0
    );
}

#[test]
fn direct_wasm_execute_split_timeout_fails_with_timeout_error() {
    let components_dir = direct_e2e_components_dir();

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
    let components_dir = direct_e2e_components_dir();

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
    let components_dir = direct_e2e_components_dir();
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

// A [WaitForSignal -> durable Delay -> Finish] workflow whose signal was
// consumed and whose execution moved past the wait must, when the environment
// is drained and the instance replays from the entry point, re-read its signal
// and complete — not dead-hang on a destructively-deleted signal, and not fire
// a spurious WAIT_TIMEOUT from a recomputed deadline. Modeled with the two-run
// preloaded-checkpoints seam: run 1 executes fresh; run 2 replays with run 1's
// durable state present and the signal still retained.
#[test]
fn direct_wasm_execute_wait_delay_finish_resumes_after_drain() {
    let components_dir = direct_e2e_components_dir();
    let workflow_id = "direct-wasm-execute-wait-delay-resume";
    let signal = serde_json::json!({ "approved": true });

    // Run 1: the wait consumes the delivered approval, persists its absolute
    // deadline as an 8-byte checkpoint, then the durable delay parks the
    // instance — the post-wait window a real drain would land in.
    let first = run_wait_workflow(
        &components_dir,
        workflow_id,
        WAIT_DELAY_FINISH,
        b"{}",
        Vec::new(),
        vec![signal.clone()],
    );
    assert!(
        first.status_success,
        "run 1 should complete on the delivered-signal path; stderr: {}",
        first.stderr
    );
    assert_eq!(
        first.output_json,
        Some(serde_json::json!({ "approved": true })),
    );
    assert!(
        first.custom_signal_polls >= 1,
        "run 1 wait must have read the delivered signal"
    );

    // The wait persisted exactly one 8-byte absolute-deadline checkpoint under
    // its deterministic signal id (the timeout-drift fix). Pre-fix, the wait
    // emitted no checkpoint at all.
    let deadline: Vec<_> = first
        .checkpoints
        .iter()
        .filter(|cp| cp.state.len() == 8)
        .collect();
    assert_eq!(
        deadline.len(),
        1,
        "run 1 should persist one 8-byte wait deadline checkpoint; saw: {:?}",
        first.checkpoints
    );
    assert!(
        deadline[0].checkpoint_id.ends_with("/wait"),
        "deadline checkpoint must be keyed by the wait's deterministic signal id, got: {}",
        deadline[0].checkpoint_id
    );

    // The durable state a resume would find committed: the wait deadline.
    let preloaded: Vec<(String, Vec<u8>)> = first
        .checkpoints
        .iter()
        .filter(|cp| !cp.state.is_empty())
        .map(|cp| (cp.checkpoint_id.clone(), cp.state.clone()))
        .collect();

    // Run 2: replay from the entry point with the deadline preloaded and the
    // signal still present (non-destructive retention).
    let second = run_wait_workflow(
        &components_dir,
        workflow_id,
        WAIT_DELAY_FINISH,
        b"{}",
        preloaded,
        vec![signal.clone()],
    );
    assert!(
        second.status_success,
        "resume must complete, not hang or time out; stderr: {}",
        second.stderr
    );
    assert_eq!(
        second.output_json,
        Some(serde_json::json!({ "approved": true })),
        "resume must reproduce the delivered-signal result (no spurious WAIT_TIMEOUT)"
    );
    assert!(
        second.custom_signal_polls >= 1,
        "resume must re-poll and re-read the retained signal"
    );
    // The deadline was read from its checkpoint, not recomputed and re-saved.
    assert!(
        second.checkpoints.iter().all(|cp| cp.state.len() != 8),
        "resume must hit the preloaded deadline checkpoint, not re-save one: {:?}",
        second.checkpoints
    );
}

// [WaitForSignal -> WaitForSignal -> Finish]: after wait1 consumes its signal,
// a drain replays from the entry point back through wait1, which must re-read
// its already-consumed signal rather than re-poll a deleted one. Both waits'
// signals must survive the replay.
#[test]
fn direct_wasm_execute_wait_wait_finish_resumes_after_drain() {
    let components_dir = direct_e2e_components_dir();
    let workflow_id = "direct-wasm-execute-wait-wait-resume";
    let signal = serde_json::json!({ "approved": true });

    let first = run_wait_workflow(
        &components_dir,
        workflow_id,
        WAIT_WAIT_FINISH,
        b"{}",
        Vec::new(),
        vec![signal.clone()],
    );
    assert!(
        first.status_success,
        "run 1 should complete once both signals are read; stderr: {}",
        first.stderr
    );
    assert_eq!(
        first.output_json,
        Some(serde_json::json!({ "first": true, "second": true })),
    );
    assert!(
        first.custom_signal_polls >= 2,
        "both waits must read a signal on run 1; polls: {}",
        first.custom_signal_polls
    );

    // Resume: replay from the entry point with the signals still retained. Both
    // waits re-read and the workflow completes identically.
    let second = run_wait_workflow(
        &components_dir,
        workflow_id,
        WAIT_WAIT_FINISH,
        b"{}",
        Vec::new(),
        vec![signal.clone()],
    );
    assert!(
        second.status_success,
        "resume must complete, not hang on a consumed signal; stderr: {}",
        second.stderr
    );
    assert_eq!(
        second.output_json,
        Some(serde_json::json!({ "first": true, "second": true })),
        "resume must reproduce both delivered-signal results"
    );
}

#[test]
fn direct_wasm_execute_durable_agent_invokes_and_saves_checkpoint() {
    let components_dir = direct_e2e_components_dir();
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
    let components_dir = direct_e2e_components_dir();
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
    let components_dir = direct_e2e_components_dir();
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
    let components_dir = direct_e2e_components_dir();
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
fn direct_wasm_execute_resolves_negative_array_index() {
    let components_dir = direct_e2e_components_dir();
    // SYN-448 regression: negative array indices must resolve Python-style at
    // runtime (`-1` = last element) instead of silently returning null. The
    // out-of-range negative (`-9`) falls through to the mapping default.
    let result = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-negative-index",
        NEGATIVE_INDEX_REFERENCE,
        br#"{"data":{"items":["a","b","c"]}}"#,
    );
    assert_eq!(
        result,
        serde_json::json!({
            "last": "c",
            "second": "b",
            "first_neg": "a",
            "first_pos": "a",
            "oob": "fallback"
        })
    );
}

#[test]
fn direct_wasm_execute_template_tojson_filter() {
    let components_dir = direct_e2e_components_dir();
    // SYN-449: the `tojson` filter (minijinja `json` feature) must be available in
    // the compiled WASM mapping engine. Output is compact JSON.
    let result = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-template-tojson",
        TEMPLATE_TOJSON_FILTER,
        br#"{"data":{"obj":{"a":1,"b":[2,3]}}}"#,
    );
    assert_eq!(
        result,
        serde_json::json!({ "json_str": "{\"a\":1,\"b\":[2,3]}" })
    );
}

#[test]
fn direct_wasm_execute_durable_agent_uses_cached_checkpoint() {
    let components_dir = direct_e2e_components_dir();
    let workflow_id = "direct-wasm-execute-agent-cached-replay";
    let checkpoint_id = format!("{workflow_id}::agent::utils::return-input::agent");

    let captured = run_direct_workflow_capture_with_preloaded_checkpoints(
        &components_dir,
        workflow_id,
        AGENT_CACHED_REPLAY,
        br#"{"value":"fresh-agent"}"#,
        false,
        vec![(checkpoint_id.clone(), br#""cached-agent""#.to_vec())],
        Vec::new(),
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
    let components_dir = direct_e2e_components_dir();

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

/// The reporter's exact bug: a downstream step references a Split's array output
/// by a NAMED KEY (`steps.split.outputs.result`) instead of indexing it or using
/// the bare array. This used to silently resolve to null and produce a green
/// (but wrong) run; it must now fail loud at runtime. This proves the fix
/// end-to-end (compile -> execute -> observe failure), not just in the resolver
/// unit tests.
#[test]
fn direct_wasm_execute_named_key_into_split_array_output_fails_loud() {
    let components_dir = direct_e2e_components_dir();

    // Same graph as `split_workflow`, but the outer Finish reaches into the
    // Split's collected ARRAY with a field name that does not exist on an array.
    let graph = SPLIT_WORKFLOW.replace("\"steps.split.outputs\"", "\"steps.split.outputs.result\"");
    assert!(
        graph.contains("steps.split.outputs.result"),
        "fixture shape changed — the bad-reference injection no longer applies"
    );

    let failure = run_direct_workflow_expect_failure(
        &components_dir,
        "direct-wasm-execute-split-bad-output-ref",
        &graph,
        br#"{"items":[{"value":1},{"value":2}]}"#,
    );

    // The error must name the offending reference, not silently swallow it.
    let error_text = serde_json::to_string(&failure.error_json).unwrap_or_default();
    assert!(
        error_text.contains("steps.split.outputs.result"),
        "failure must attribute the bad reference; got: {error_text}"
    );
}

#[test]
fn direct_wasm_execute_value_switch_finish_reports_completion() {
    let components_dir = direct_e2e_components_dir();

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
    let components_dir = direct_e2e_components_dir();

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
    let components_dir = direct_e2e_components_dir();

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
    let components_dir = direct_e2e_components_dir();

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
    let components_dir = direct_e2e_components_dir();

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
    let components_dir = direct_e2e_components_dir();

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

// ===========================================================================
// Embedded execution — runtara-component-host's WorkflowExecutor instead of
// the wasmtime CLI process. Same in-process compile path, same hermetic core
// stub; proves the composed component behaves identically under the embedded
// engine before the runner migration switches over to it.
// ===========================================================================

fn embedded_executor() -> &'static runtara_component_host::WorkflowExecutor {
    static EXECUTOR: std::sync::OnceLock<runtara_component_host::WorkflowExecutor> =
        std::sync::OnceLock::new();
    EXECUTOR.get_or_init(|| {
        let engine =
            runtara_component_host::build_engine(&runtara_component_host::EngineConfig::default())
                .expect("build embedded engine");
        runtara_component_host::spawn_epoch_ticker(Arc::clone(&engine));
        runtara_component_host::WorkflowExecutor::new(engine).expect("build workflow executor")
    })
}

fn run_direct_workflow_embedded(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    workflow_input: &[u8],
) -> CapturedRun {
    let temp = tempfile::tempdir().expect("tempdir");
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    // This section A/Bs the SAME artifact between the embedded executor and
    // the wasmtime CLI, so it pins the legacy Composed binding — the only
    // shape the CLI can run.
    let compiled = compile_direct_workflow_composed_with_binding(
        DirectCompilationInput {
            workflow_id: workflow_id.to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        },
        components_dir,
        RuntimeBinding::Composed,
    )
    .expect("direct composed compile");

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let (capture_tx, capture_rx) = mpsc::channel::<CapturedMessage>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let input_arc = Arc::new(workflow_input.to_vec());
    let server_state = Arc::new(ServerState::default());
    let server_state_for_assertions = server_state.clone();
    let server_handle =
        thread::spawn(move || serve(listener, capture_tx, server_state, stop_rx, input_arc));

    // Same env contract the CLI variant passes via --env flags.
    let mut env = HashMap::new();
    env.insert("RUNTARA_HTTP_URL".to_string(), format!("http://{addr}"));
    env.insert(
        "RUNTARA_HTTP_PROXY_URL".to_string(),
        format!("http://{addr}/llm-proxy"),
    );
    env.insert(
        "RUNTARA_OBJECT_MODEL_URL".to_string(),
        format!("http://{addr}/object-model"),
    );
    env.insert("RUNTARA_SERVER_ADDR".to_string(), addr.to_string());
    env.insert("RUNTARA_INSTANCE_ID".to_string(), workflow_id.to_string());
    env.insert(
        "RUNTARA_TENANT_ID".to_string(),
        "direct-wasm-execute".to_string(),
    );
    env.insert("RUST_LOG".to_string(), "warn".to_string());

    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let result = runtime.block_on(async {
        let pre = executor
            .load(&compiled.wasm_path)
            .await
            .expect("load composed workflow component");
        executor
            .execute(
                &pre,
                runtara_component_host::WorkflowRunSpec {
                    env,
                    stderr: None,
                    timeout: Duration::from_secs(120),
                    cancel: None,
                    limits: runtara_component_host::WorkflowLimits::default(),
                    runtime: None,
                },
            )
            .await
    });

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
    let llm_requests = server_state_for_assertions
        .llm_requests
        .lock()
        .expect("llm_requests lock")
        .clone();
    let sql_requests = server_state_for_assertions
        .sql_requests
        .lock()
        .expect("sql_requests lock")
        .clone();
    let custom_signal_polls = *server_state_for_assertions
        .custom_signal_polls
        .lock()
        .expect("custom_signal_polls lock");
    let stderr = match &result.exit {
        runtara_component_host::WorkflowExit::Failed { reason } => reason.clone(),
        _ => String::new(),
    };
    CapturedRun {
        output_json,
        error_json,
        events,
        sleeps,
        checkpoints,
        llm_requests,
        sql_requests,
        custom_signal_polls,
        status_success: matches!(result.exit, runtara_component_host::WorkflowExit::Completed),
        stderr,
        memory_peak_bytes: Some(result.memory_peak_bytes),
    }
}

#[test]
fn embedded_execute_finish_passthrough_reports_completion() {
    let components_dir = direct_e2e_components_dir();

    let captured = run_direct_workflow_embedded(
        &components_dir,
        "embedded-finish-passthrough",
        SIMPLE_PASSTHROUGH,
        br#"{"input":"direct-finish"}"#,
    );

    assert!(
        captured.status_success,
        "embedded run failed: {}",
        captured.stderr
    );
    assert_eq!(
        captured.output_json,
        Some(serde_json::json!({ "result": "direct-finish" }))
    );
    assert!(captured.error_json.is_none());
}

#[test]
fn embedded_execute_error_workflow_reports_failure() {
    let components_dir = direct_e2e_components_dir();

    let captured = run_direct_workflow_embedded(
        &components_dir,
        "embedded-error",
        ERROR_DIRECT_SIMPLE,
        br#"{"requestId":"req-123"}"#,
    );

    assert!(
        !captured.status_success,
        "Error workflow must surface a failed run result"
    );
    assert!(
        captured.output_json.is_none(),
        "Error workflow must not POST /completed"
    );
    assert_eq!(
        captured.error_json,
        Some(serde_json::json!({
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
        }))
    );
}

#[test]
fn embedded_execute_is_repeatable_across_runs() {
    let components_dir = direct_e2e_components_dir();

    // Two full runs back to back: each gets a fresh Store from the shared
    // executor, so state must not leak between instances.
    for round in 0..2 {
        let captured = run_direct_workflow_embedded(
            &components_dir,
            &format!("embedded-repeat-{round}"),
            SIMPLE_PASSTHROUGH,
            br#"{"input":"direct-finish"}"#,
        );
        assert!(
            captured.status_success,
            "round {round} failed: {}",
            captured.stderr
        );
        assert_eq!(
            captured.output_json,
            Some(serde_json::json!({ "result": "direct-finish" })),
            "round {round} output mismatch"
        );
    }
}

// ===========================================================================
// Regression: large per-iteration scope state exhausts the workflow guest heap.
//
// The hand-emitted workflow core module allocates via a bump pointer that never
// frees (compile/core_module.rs `export_realloc`) and its canonical-ABI
// post-return is a no-op, so every `list<u8>` a host call returns into workflow
// memory is leaked for the life of the run. A Split copies the whole parent
// scope into each iteration (`split_iteration_variables`) and rebuilds the
// iteration source (`build_source`); when the scope carries a large value, every
// iteration leaks several multi-MB buffers, so guest heap climbs ~linearly with
// iteration count and eventually crosses the per-instance memory cap — a guest
// OOM trap surfaced as `WorkflowExit::Failed { "guest memory limit exceeded" }`.
//
// Same graph, same iteration count, same cap: a large scope variable traps while
// a tiny one completes — isolating the per-iteration scope buffers as the cause.
// ===========================================================================

/// A sequential Split that fans out over `data.items`. Each iteration's subgraph
/// is a single Finish (no inner agent) so the loop makes NO per-iteration HTTP
/// call — that keeps long runs clear of the harness's per-request flake, which
/// would otherwise truncate a 300-iteration run before the leak accrues. The
/// per-iteration leak under test (`split_iteration_variables` + `build_source`
/// re-materializing the scope) happens regardless of the subgraph body. The Split
/// declares an iteration variable `big` whose value is an immediate string baked
/// into the compiled workflow's static data, copied into every iteration's scope;
/// the test sizes `big` via `scope_bytes`.
fn split_scope_leak_graph(scope_bytes: usize) -> String {
    let big = "a".repeat(scope_bytes);
    let graph = serde_json::json!({
        "durable": false,
        "steps": {
            "split": {
                "stepType": "Split",
                "id": "split",
                "name": "Fan Out",
                "config": {
                    "value": { "valueType": "reference", "value": "data.items" },
                    "sequential": true,
                    "variables": {
                        "big": { "valueType": "immediate", "value": big }
                    }
                },
                "subgraph": {
                    "name": "Item",
                    "entryPoint": "finish",
                    "steps": {
                        "finish": {
                            "stepType": "Finish",
                            "id": "finish",
                            "inputMapping": {
                                "ok": { "valueType": "immediate", "value": true }
                            }
                        }
                    },
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
        "inputSchema": { "items": { "type": "array" } },
        "outputSchema": {}
    });
    serde_json::to_string(&graph).expect("graph serializes")
}

/// Small input envelope: just the iteration list of length `n`. The large scope
/// state lives in the graph, not here, so this stays well under any input cap.
fn split_scope_leak_input(n: usize) -> Vec<u8> {
    let items: Vec<Value> = (0..n).map(|i| serde_json::json!(i)).collect();
    let input = serde_json::json!({ "data": { "items": items }, "variables": {} });
    serde_json::to_vec(&input).expect("input serializes")
}

/// Shared sizing for the scope-leak pair: a large in-scope value, enough
/// iterations for the per-iteration leak to dominate, and a generous (realistic)
/// guest memory cap. The cap is far above the baseline machinery's footprint, so
/// the only thing that can exhaust it is the leak — and the only variable between
/// the two tests is the scope size.
// Scope kept at 512 KiB (not multi-MB) so the stdlib's per-call parse peak and the
// HTTP body sizes stay small — isolating the *workflow* heap leak as the only thing
// that can exhaust the cap, and keeping the run clear of the harness's large-body
// flake. With 300 iterations and a 64 MiB cap, the un-reclaimed per-iteration
// buffers (~1+ MiB each) would OOM by ~iteration 50 without the arena reset; with
// it, heap stays flat and the loop completes.
const SPLIT_LEAK_SCOPE_BYTES: usize = 512 * 1024;
const SPLIT_LEAK_ITEMS: usize = 300;
const SPLIT_LEAK_MEM_CAP_BYTES: usize = 64 * 1024 * 1024;

/// Regression for the per-iteration scope leak: a large in-scope variable is
/// copied into every Split iteration, and the workflow core module's bump
/// allocator never frees (post-return is a no-op), so guest heap climbs without
/// bound and the run dies mid-Split — as the silent
/// `WorkflowExit::Failed { "guest memory limit exceeded" }` once the cap is
/// crossed, or (at a higher cap) an `HttpProtocolError` once a runaway buffer
/// breaks an outbound call. Both are the production regression.
///
/// Asserts the FIXED behavior — guest memory stays bounded across the iterations
/// (it would OOM at the cap without the per-iteration arena reset).
/// [`split_small_scope_completes_under_same_cap`] is the same graph with a tiny
/// scope, isolating scope size as the cause.
#[test]
fn split_large_scope_does_not_exhaust_guest_heap() {
    let components_dir = direct_e2e_components_dir();

    let graph = split_scope_leak_graph(SPLIT_LEAK_SCOPE_BYTES);
    let input = split_scope_leak_input(SPLIT_LEAK_ITEMS);

    // track_events is off: the per-iteration leak is driven by the stdlib calls
    // that copy/rebuild the scope (intra-guest), so it accumulates without large
    // event POSTs. (With events on, the Split's own debug payload would itself
    // carry the multi-MB `config.variables` — a related flooding bug.)
    let captured = run_direct_workflow_capture_full(
        &components_dir,
        "split-large-scope-leak",
        &graph,
        &input,
        false,
        Vec::new(),
        Vec::new(),
        vec![(
            "RUNTARA_INSTANCE_MEMORY_MAX_BYTES".into(),
            SPLIT_LEAK_MEM_CAP_BYTES.to_string(),
        )],
    );

    // Assert on the guest memory peak rather than completion: the arena reset's
    // guarantee is *bounded* heap, and that signal is immune to the harness's
    // load-sensitive HTTP flake (which can fail a run regardless of memory).
    // Without the reset, 300 un-reclaimed iterations (~1+ MiB each) would climb
    // well past this bound and OOM at the 64 MiB cap; with it, the peak stays near
    // one iteration's footprint (a few MiB).
    let peak = captured
        .memory_peak_bytes
        .expect("embedded executor reports a memory peak");
    assert!(
        peak < 32 * 1024 * 1024,
        "per-iteration heap not reclaimed: peak {peak} bytes over {} iterations \
         (expected bounded to ~one iteration)",
        SPLIT_LEAK_ITEMS,
    );
    assert!(
        !captured.stderr.contains("guest memory limit exceeded"),
        "Split exhausted guest memory mid-loop: {}",
        captured.stderr,
    );
}

/// Control: the same graph, iteration count, and cap with a tiny in-scope value
/// completes cleanly — proving the failure is driven by per-iteration scope size,
/// not the Split structure or iteration count.
#[test]
fn split_small_scope_completes_under_same_cap() {
    let components_dir = direct_e2e_components_dir();

    let graph = split_scope_leak_graph(8); // 8-byte scope variable
    let input = split_scope_leak_input(SPLIT_LEAK_ITEMS);

    let captured = run_direct_workflow_capture_full(
        &components_dir,
        "split-small-scope-ok",
        &graph,
        &input,
        false,
        Vec::new(),
        Vec::new(),
        vec![(
            "RUNTARA_INSTANCE_MEMORY_MAX_BYTES".into(),
            SPLIT_LEAK_MEM_CAP_BYTES.to_string(),
        )],
    );

    assert!(
        captured.status_success,
        "small-scope split should complete under the same cap; stderr:\n{}",
        captured.stderr
    );
}

/// With step events ON, a Split's own `step_debug_start` payload used to embed the
/// fully-resolved `value` (the entire list it fans out over) and `variables` (large
/// in-scope references), so a large scope flooded the event stream and broke the
/// event POST before any iteration ran. `bounded_debug_value` now summarizes those
/// fields, so the same large-scope Split runs to completion with events enabled.
#[test]
fn split_large_scope_with_events_does_not_flood_debug() {
    let components_dir = direct_e2e_components_dir();

    // Few iterations: this exercises the Split's once-per-step debug payload with a
    // large in-scope value, not the per-iteration leak (covered by the test above).
    let graph = split_scope_leak_graph(SPLIT_LEAK_SCOPE_BYTES);
    let input = split_scope_leak_input(8);

    let captured = run_direct_workflow_capture_full(
        &components_dir,
        "split-large-scope-events",
        &graph,
        &input,
        true, // track_events on: exercises the Split's debug payload
        Vec::new(),
        Vec::new(),
        vec![(
            "RUNTARA_INSTANCE_MEMORY_MAX_BYTES".into(),
            SPLIT_LEAK_MEM_CAP_BYTES.to_string(),
        )],
    );

    assert!(
        captured.status_success,
        "large-scope split with events should complete (bounded debug payload); \
         stderr={:?} error_json={:?}",
        captured.stderr, captured.error_json,
    );
}

/// A While whose accumulator grows by one chunk per iteration: each pass wraps the
/// previous output (`variables._previousOutputs`) and appends a fresh `chunk_bytes`
/// string, so after `k` iterations the carried state is a `k`-deep nest of size
/// ~`k * chunk_bytes`. The chunk is sized above the 16 KiB intern threshold, so
/// every iteration's `_previousOutputs` is a *distinct, larger* blob interned to a
/// `$wfref` handle (content-dedup can't help a growing value), and the per-reset
/// `value-store-retain` GC runs between each pass. The condition runs the loop
/// exactly `data.count` times. The final Finish emits only `iterations` so the
/// completion path stays tiny regardless of accumulator size.
fn while_accumulator_graph(chunk_bytes: usize) -> String {
    let chunk = "a".repeat(chunk_bytes);
    let graph = serde_json::json!({
        "durable": false,
        "steps": {
            "loop": {
                "stepType": "While",
                "id": "loop",
                "name": "Grow Accumulator",
                "condition": {
                    "type": "operation",
                    "op": "LT",
                    "arguments": [
                        { "valueType": "reference", "value": "loop.index" },
                        { "valueType": "reference", "value": "data.count" }
                    ]
                },
                "subgraph": {
                    "name": "Append Chunk",
                    "entryPoint": "finish",
                    "steps": {
                        "finish": {
                            "stepType": "Finish",
                            "id": "finish",
                            "inputMapping": {
                                // Wrap the prior accumulator and append a fresh
                                // large chunk: the carried state grows by one
                                // chunk per iteration.
                                "prev": {
                                    "valueType": "reference",
                                    "value": "variables._previousOutputs",
                                    "default": null
                                },
                                "chunk": { "valueType": "immediate", "value": chunk }
                            }
                        }
                    },
                    "executionPlan": []
                },
                "config": { "maxIterations": 1000 }
            },
            "finish": {
                "stepType": "Finish",
                "id": "finish",
                "inputMapping": {
                    "iterations": {
                        "valueType": "reference",
                        "value": "steps.loop.outputs.iterations"
                    }
                }
            }
        },
        "entryPoint": "loop",
        "executionPlan": [
            { "fromStep": "loop", "toStep": "finish" }
        ],
        "variables": {},
        "inputSchema": { "count": { "type": "number" } },
        "outputSchema": {}
    });
    serde_json::to_string(&graph).expect("graph serializes")
}

fn while_accumulator_input(iterations: usize) -> Vec<u8> {
    let input = serde_json::json!({ "data": { "count": iterations }, "variables": {} });
    serde_json::to_vec(&input).expect("input serializes")
}

/// A While whose iteration body reads the previous iteration's `loop.outputs`
/// through a **template**, while the iteration output is padded past the 16 KiB
/// intern threshold so `loop.outputs` is carried by `$wfref` handle on
/// iteration 1+. Mirrors the production regression where a paginating loop read
/// `{% if loop.outputs.next_page %}…` in a templated agent input.
fn while_template_reads_loop_outputs_graph(chunk_bytes: usize) -> String {
    let chunk = "a".repeat(chunk_bytes);
    let graph = serde_json::json!({
        "durable": false,
        "steps": {
            "loop": {
                "stepType": "While",
                "id": "loop",
                "name": "Read Loop Outputs",
                "condition": {
                    "type": "operation",
                    "op": "LT",
                    "arguments": [
                        { "valueType": "reference", "value": "loop.index" },
                        { "valueType": "reference", "value": "data.count" }
                    ]
                },
                "subgraph": {
                    "name": "Iter",
                    "entryPoint": "finish",
                    "steps": {
                        "finish": {
                            "stepType": "Finish",
                            "id": "finish",
                            "inputMapping": {
                                // The reported pattern: a template reaching into
                                // the prior iteration's loop outputs. Renders "1"
                                // on iteration 0 (loop.outputs is null) and the
                                // next_page value once it is set.
                                "page": {
                                    "valueType": "template",
                                    "value": "{% if loop.outputs.next_page %}{{ loop.outputs.next_page }}{% else %}1{% endif %}"
                                },
                                "next_page": { "valueType": "immediate", "value": 7 },
                                // Pads the iteration output over the intern
                                // threshold so loop.outputs becomes a handle.
                                "chunk": { "valueType": "immediate", "value": chunk }
                            }
                        }
                    },
                    "executionPlan": []
                },
                "config": { "maxIterations": 1000 }
            },
            "finish": {
                "stepType": "Finish",
                "id": "finish",
                "inputMapping": {
                    "iterations": {
                        "valueType": "reference",
                        "value": "steps.loop.outputs.iterations"
                    },
                    "page": {
                        "valueType": "reference",
                        "value": "steps.loop.outputs.outputs.page"
                    }
                }
            }
        },
        "entryPoint": "loop",
        "executionPlan": [
            { "fromStep": "loop", "toStep": "finish" }
        ],
        "variables": {},
        "inputSchema": { "count": { "type": "number" } },
        "outputSchema": {}
    });
    serde_json::to_string(&graph).expect("graph serializes")
}

/// Regression for the interning-handle template opacity (8.0.19): once a While's
/// accumulated `loop.outputs` crosses the 16 KiB intern threshold it is carried
/// as a `$wfref` handle. References saw through it, but template rendering did
/// not — so `{% if loop.outputs.next_page %}` raised "Template render error:
/// undefined value" on iteration 1 and crashed the loop. With the fix the
/// template renders against a materialized source and the loop completes.
#[test]
fn direct_wasm_execute_while_template_reads_interned_loop_outputs() {
    let components_dir = direct_e2e_components_dir();

    // 20 KiB chunk — just over the 16 KiB threshold so the iteration output is
    // interned, three iterations to read it back at least twice.
    let graph = while_template_reads_loop_outputs_graph(20 * 1024);

    let output = run_direct_workflow(
        &components_dir,
        "while-template-reads-interned-loop-outputs",
        &graph,
        br#"{"count":3}"#,
    );

    assert_eq!(
        output["iterations"], 3,
        "loop must run to completion (pre-fix it crashed on iteration 1)"
    );
    assert_eq!(
        output["page"], "7",
        "template must read next_page through the interned loop.outputs handle"
    );
}

// 64 KiB chunk (above the 16 KiB intern threshold) appended over 60 iterations.
// Final accumulator ~3.8 MiB. The two cost terms diverge sharply, which makes the
// guest-memory peak a clean GC signal:
//   * One iteration's working set — materialize the accumulator at the Finish
//     boundary plus serde scratch — is O(current accumulator), the same with or
//     without GC. This sets the *GC'd* peak (empirically ~28 MiB at 50 iters,
//     ~34 MiB at 60).
//   * The *persistent* interned store, if never swept, accumulates the distinct
//     blobs of size 1·chunk, 2·chunk, … N·chunk → Σ k·64KiB for k=1..60 ≈ 114 MiB,
//     past the 96 MiB cap. So without `value-store-retain` the store alone OOMs
//     mid-loop (the production regression); with it only the current accumulator
//     survives each reset, so the peak stays the working-set term, under the
//     48 MiB assertion.
const WHILE_ACC_CHUNK_BYTES: usize = 64 * 1024;
const WHILE_ACC_ITERATIONS: usize = 60;
const WHILE_ACC_MEM_CAP_BYTES: usize = 96 * 1024 * 1024;

/// Regression for the growing-accumulator While: the per-reset `value-store-retain`
/// frees the previous iteration's superseded interned accumulator, so the host
/// value store stays O(N) instead of O(N²) and the guest memory peak stays bounded
/// to the per-iteration working set.
///
/// Like the Split scope-leak tests, this asserts on `memory_peak_bytes`, NOT
/// completion: a While issues a per-iteration `heartbeat`/`check-signals`/`now-ms`
/// HTTP round-trip to the mock runtime, and that path carries the harness's
/// documented load-sensitive HTTP flake — so requiring the run to finish would make
/// the test flaky (under load even the index-only While fails to complete). The
/// peak is flake-immune in the right direction: an early HTTP death only *lowers*
/// the peak (test still passes), while a GC regression (linear → O(N²)) drives the
/// peak past the cap and OOMs. The deterministic proof that the GC call is wired
/// lives in `direct_core_emits_value_store_retain_for_loops`, the intern/materialize
/// round-trip is covered by the stdlib `value_store_retain_*` and `lookup_resolves_*`
/// unit tests; this is the end-to-end backstop.
#[test]
fn while_growing_accumulator_stays_bounded() {
    let components_dir = direct_e2e_components_dir();

    let graph = while_accumulator_graph(WHILE_ACC_CHUNK_BYTES);
    let input = while_accumulator_input(WHILE_ACC_ITERATIONS);

    let captured = run_direct_workflow_capture_full(
        &components_dir,
        "while-growing-accumulator",
        &graph,
        &input,
        false,
        Vec::new(),
        Vec::new(),
        vec![(
            "RUNTARA_INSTANCE_MEMORY_MAX_BYTES".into(),
            WHILE_ACC_MEM_CAP_BYTES.to_string(),
        )],
    );

    // Without the GC the un-reclaimed O(N²) interned blobs climb past this bound and
    // OOM at the 96 MiB cap; with it the peak is just one iteration's working set.
    // 48 MiB sits above the GC'd working set and far below the GC-regression peak,
    // so a linear→quadratic regression trips it while a healthy run — or an early
    // HTTP flake — passes.
    let peak = captured
        .memory_peak_bytes
        .expect("embedded executor reports a memory peak");
    assert!(
        peak < 48 * 1024 * 1024,
        "accumulator not reclaimed across iterations: peak {peak} bytes over {} \
         iterations (expected bounded to one accumulator's working set)",
        WHILE_ACC_ITERATIONS,
    );
    assert!(
        !captured.stderr.contains("guest memory limit exceeded"),
        "While exhausted guest memory mid-loop (accumulator not GC'd): {}",
        captured.stderr,
    );
}

// ============================================================================
// Raw SQL retry semantics (query-sql / execute-sql)
// ============================================================================

/// One-step graph driving an object-model SQL capability at the scripted mock.
/// `retry_delay` is 1ms so exhausting retries doesn't slow the suite.
fn raw_sql_step_graph(capability_id: &str, max_retries: u32) -> String {
    serde_json::json!({
        "name": "raw-sql-retry",
        "entryPoint": "sqlstep",
        "executionPlan": [{"fromStep": "sqlstep", "toStep": "finish"}],
        "steps": {
            "sqlstep": {
                "id": "sqlstep", "stepType": "Agent", "name": "SQL",
                "agentId": "object-model", "capabilityId": capability_id,
                "connectionId": "conn-1",
                "maxRetries": max_retries, "retryDelay": 1,
                "inputMapping": {
                    "sql": {"valueType": "immediate", "value": "SELECT 1 AS one"}
                }
            },
            "finish": {
                "id": "finish", "stepType": "Finish",
                "inputMapping": {
                    "rows_affected": {"valueType": "reference", "value": "steps.sqlstep.outputs.rows_affected"}
                }
            }
        }
    })
    .to_string()
}

fn sql_error_body(msg: &str) -> Value {
    serde_json::json!({"success": false, "error": msg})
}

#[test]
fn direct_wasm_execute_sql_5xx_is_permanent_zero_retries() {
    let components_dir = direct_e2e_components_dir();

    // A 5xx on a write means the statement outcome on the tenant DB is
    // unknown — the agent downgrades check_status's transient classification
    // to permanent and the runtime must NOT retry. The scripted success is
    // never consumed; exactly one request reaches the mock.
    let captured = run_direct_workflow_capture_full_sql(
        &components_dir,
        "execute-sql-5xx-permanent",
        &raw_sql_step_graph("execute-sql", 3),
        br#"{}"#,
        false,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![
            (500, sql_error_body("upstream boom")),
            (200, serde_json::json!({"success": true, "rowsAffected": 1})),
        ],
        Vec::new(),
    );

    assert!(
        !captured.status_success,
        "execute-sql must fail on 5xx, not retry into the scripted success; output: {:?}",
        captured.output_json
    );
    assert_eq!(
        captured.sql_requests.len(),
        1,
        "execute-sql must never auto-retry a server error (double-apply risk): {:?}",
        captured.sql_requests
    );
    let error = captured
        .error_json
        .map(|e| e.to_string())
        .unwrap_or_else(|| captured.stderr.clone());
    assert!(
        error.contains("OBJECT_MODEL_UPSTREAM_ERROR"),
        "failure should carry the upstream error code: {error}"
    );
}

#[test]
fn direct_wasm_query_sql_5xx_retries_then_succeeds() {
    let components_dir = direct_e2e_components_dir();

    // Reads run in a READ ONLY transaction server-side, so retrying a 5xx is
    // safe — stock transient classification stands and the runtime retries
    // into the scripted success.
    let captured = run_direct_workflow_capture_full_sql(
        &components_dir,
        "query-sql-5xx-retries",
        &raw_sql_step_graph("query-sql", 2),
        br#"{}"#,
        false,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        vec![
            (500, sql_error_body("transient boom")),
            (
                200,
                serde_json::json!({"success": true, "rows": [{"one": 1}], "rowCount": 1}),
            ),
        ],
        Vec::new(),
    );

    assert!(
        captured.status_success,
        "query-sql should retry the 5xx and succeed; stderr: {}; error: {:?}",
        captured.stderr, captured.error_json
    );
    assert_eq!(
        captured.sql_requests.len(),
        2,
        "expected exactly one retry (500 then 200): {:?}",
        captured.sql_requests
    );
}

#[test]
fn direct_wasm_sql_transport_failure_classification() {
    let components_dir = direct_e2e_components_dir();

    // Point the object-model URL at a port nothing listens on: transport
    // failure on every attempt. query-sql reclassifies transport errors to
    // transient (retries, then exhausts); execute-sql keeps them permanent
    // (the statement may have committed).
    let closed_port = {
        let probe = TcpListener::bind("127.0.0.1:0").expect("bind probe");
        let port = probe.local_addr().expect("local_addr").port();
        drop(probe);
        port
    };
    let refused_env = vec![(
        "RUNTARA_OBJECT_MODEL_URL".to_string(),
        format!("http://127.0.0.1:{closed_port}/object-model"),
    )];

    for (capability, expected_category) in
        [("query-sql", "transient"), ("execute-sql", "permanent")]
    {
        let captured = run_direct_workflow_capture_full_sql(
            &components_dir,
            &format!("{capability}-transport-refused"),
            &raw_sql_step_graph(capability, 1),
            br#"{}"#,
            false,
            Vec::new(),
            Vec::new(),
            refused_env.clone(),
            Vec::new(),
            Vec::new(),
        );

        assert!(
            !captured.status_success,
            "{capability}: refused connection must fail the step"
        );
        let error = captured
            .error_json
            .map(|e| e.to_string())
            .unwrap_or_else(|| captured.stderr.clone());
        assert!(
            error.contains("OBJECT_MODEL_HTTP_ERROR"),
            "{capability}: expected transport error code, got: {error}"
        );
        assert!(
            error.contains(&format!("\\\"category\\\":\\\"{expected_category}\\\""))
                || error.contains(&format!("\"category\":\"{expected_category}\"")),
            "{capability}: expected category {expected_category}, got: {error}"
        );
    }
}

// ===========================================================================
// Invoke ABI (Phase 3 of docs/unify-agents-workflows-plan.md): the workflow
// exports lifecycle.invoke instead of wasi:cli/run — input as the call
// argument, terminal result as the lifted return value. These are the Spike-E
// acceptance tests: the emitter's param-fold + result-area writer, the WIT
// world, ComponentEncoder validation, wac composition, and wasmtime's typed
// lift all have to agree for a single byte to come back.
// ===========================================================================

fn compile_invoke_abi_artifact(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
) -> runtara_workflows::direct_wasm::DirectCompilationResult {
    compile_invoke_abi_artifact_configured(components_dir, workflow_id, graph_json, false)
}

fn compile_invoke_abi_artifact_configured(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    store_freeing_sleep: bool,
) -> runtara_workflows::direct_wasm::DirectCompilationResult {
    compile_invoke_abi_artifact_full(
        components_dir,
        workflow_id,
        graph_json,
        store_freeing_sleep,
        false,
    )
}

fn compile_invoke_abi_artifact_full(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    store_freeing_sleep: bool,
    omit_runtime: bool,
) -> runtara_workflows::direct_wasm::DirectCompilationResult {
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    let temp = tempfile::tempdir().expect("tempdir");
    // Pin BOTH knobs: these tests assert the HostImport+invoke shape and
    // must not inherit the battery's binding/ABI axis env vars.
    let result = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: workflow_id.to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        },
        components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
        store_freeing_sleep,
        omit_runtime,
    )
    .expect("invoke-abi compile+compose succeeds");
    // Keep the tempdir alive by leaking it — the executor reads the artifact
    // lazily and the test owns the whole lifetime anyway.
    std::mem::forget(temp);
    result
}

/// A pure, non-durable workflow — a single Finish echoing the input, no
/// runtime-requiring feature. The degenerate agent case.
const PURE_PASSTHROUGH: &str = r#"{
  "name": "Pure Passthrough",
  "durable": false,
  "steps": {
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": { "result": { "valueType": "reference", "value": "data.input" } }
    }
  },
  "entryPoint": "finish",
  "executionPlan": [],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}"#;

/// Workflow-as-agent slice d: a PURE, non-durable, invoke-ABI workflow compiled
/// with the omit-runtime gate drops the `runtara:workflow-runtime/runtime`
/// import entirely and executes with NO runtime host attached — its terminal
/// result travels solely in-band. This is the composition-safe, agent-shaped
/// artifact the workflow-as-agent path builds on.
#[test]
fn direct_wasm_execute_invoke_omit_runtime_pure_workflow_runs_with_no_runtime_host() {
    let components_dir = direct_e2e_components_dir();
    let compiled = compile_invoke_abi_artifact_full(
        &components_dir,
        "omit-runtime-pure",
        PURE_PASSTHROUGH,
        false,
        true,
    );

    // Compile-side proof: the omit decision took, and the world imports no runtime.
    assert!(
        compiled.omit_runtime,
        "a pure durable:false invoke workflow must omit the runtime import"
    );
    assert!(
        !compiled
            .component_artifacts
            .world_wit
            .contains("workflow-runtime/runtime"),
        "world must not import the runtime:\n{}",
        compiled.component_artifacts.world_wit
    );

    // Runtime-side proof: it executes with NO runtime host attached, completing
    // in-band (no runtime.complete fires). Had any runtime.* call been emitted,
    // the composed artifact would reference a poisoned import index and fail
    // ComponentEncoder validation at compile — so reaching here already proves
    // zero runtime calls; running with `runtime: None` proves it at execution.
    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let run = runtime.block_on(async {
        let pre = executor
            .load_instance_pre(&compiled.wasm_path)
            .await
            .expect("load omit-runtime artifact");
        executor
            .execute_invoke(
                &pre,
                runtara_component_host::WorkflowRunSpec {
                    env: HashMap::new(),
                    stderr: None,
                    timeout: Duration::from_secs(60),
                    cancel: None,
                    limits: runtara_component_host::WorkflowLimits::default(),
                    runtime: None,
                },
                br#"{"input":"agent-shaped"}"#.to_vec(),
            )
            .await
    });
    let output = match run.exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("omit-runtime workflow must complete in-band, got {other:?}"),
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&output).expect("output is JSON"),
        serde_json::json!({ "result": "agent-shaped" })
    );

    // Control: the SAME workflow with the runtime kept still imports it and
    // returns the same output — the omit is purely a shape/side-effect change.
    let kept = compile_invoke_abi_artifact_full(
        &components_dir,
        "omit-runtime-off",
        PURE_PASSTHROUGH,
        false,
        false,
    );
    assert!(!kept.omit_runtime);
    assert!(
        kept.component_artifacts
            .world_wit
            .contains("workflow-runtime/runtime"),
        "control artifact must keep the runtime import"
    );

    // Soundness: a workflow that WOULD call runtime keeps the import even when
    // omit is requested — the needs_runtime guard makes the effective decision.
    let agentful = compile_invoke_abi_artifact_full(
        &components_dir,
        "omit-runtime-guarded",
        AGENT_CACHED_REPLAY,
        false,
        true,
    );
    assert!(
        !agentful.omit_runtime,
        "a runtime-needing workflow must keep the runtime import despite the omit request"
    );
    assert!(
        agentful
            .component_artifacts
            .world_wit
            .contains("workflow-runtime/runtime")
    );
}

fn compile_agent_capabilities_artifact(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
) -> runtara_workflows::direct_wasm::DirectCompilationResult {
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: workflow_id.to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        },
        components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
        false,
        // omit_runtime is forced true for AgentCapabilities by the compiler.
        false,
    )
    .expect("agent-capabilities compile+compose succeeds");
    std::mem::forget(temp);
    result
}

/// Workflow-as-agent slice a: a pure workflow compiled with the
/// `AgentCapabilities` ABI exports `runtara:agent-<id>/capabilities.invoke(
/// capability-id, input, connection) -> result<list<u8>, error-info>` — the
/// exact agent shape — and is invocable AS an agent through a wasmtime typed
/// call. This exercises the 17-param fold, the `result<list<u8>, error-info>`
/// return layout, and the export naming end to end (a wrong local layout or
/// return offset would surface as a typed-call trap or a wrong payload).
#[test]
fn direct_wasm_execute_agent_capabilities_workflow_invocable_as_agent() {
    let components_dir = direct_e2e_components_dir();
    let compiled =
        compile_agent_capabilities_artifact(&components_dir, "workflow-as-agent", PURE_PASSTHROUGH);

    // Shape: agent-shaped export, zero runtime imports.
    assert!(
        compiled.omit_runtime,
        "AgentCapabilities implies omit-runtime"
    );
    let world = &compiled.component_artifacts.world_wit;
    assert!(
        world.contains("export runtara:agent-workflow-agent/capabilities@0.3.0"),
        "world must export the capabilities interface:\n{world}"
    );
    assert!(
        !world.contains("workflow-runtime/runtime"),
        "agent-shaped workflow must import no runtime:\n{world}"
    );

    // Invoke it AS an agent: capabilities.invoke(cap-id, input).
    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let result = runtime.block_on(async {
        let pre = executor
            .load_instance_pre(&compiled.wasm_path)
            .await
            .expect("load agent-capabilities artifact");
        executor
            .invoke_capability(
                &pre,
                "runtara:agent-workflow-agent/capabilities@0.3.0",
                "invoke",
                br#"{"input":"as-agent"}"#.to_vec(),
            )
            .await
            .expect("capability invocation runs")
    });
    let output = result.expect("capability returns Ok(list<u8>)");
    assert_eq!(
        serde_json::from_slice::<Value>(&output).expect("output is JSON"),
        serde_json::json!({ "result": "as-agent" }),
        "the workflow-as-agent must transform input exactly as it does as a workflow"
    );
}

/// The non-suspending gate: a workflow that would call the runtime (here a
/// durable Delay) is rejected when compiled as an agent, rather than silently
/// producing a poisoned import — the agent capability shape cannot suspend.
#[test]
fn direct_wasm_execute_agent_capabilities_rejects_runtime_needing_workflow() {
    let components_dir = direct_e2e_components_dir();
    let graph: ExecutionGraph =
        serde_json::from_str(STORE_FREEING_DELAY).expect("delay fixture parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let err = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "delay-not-agent".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
        },
        &components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
        false,
        false,
    )
    .expect_err("a durable-delay workflow must be rejected as an agent");
    assert!(
        format!("{err}").contains("not agent-eligible"),
        "unexpected error: {err}"
    );
}

#[test]
fn direct_wasm_execute_invoke_abi_returns_completed_outcome_in_band() {
    let components_dir = direct_e2e_components_dir();
    let compiled =
        compile_invoke_abi_artifact(&components_dir, "invoke-abi-completed", SIMPLE_PASSTHROUGH);

    // Input travels as the call argument — the RecordingRuntimeHost's
    // load_input must never be consulted (poisoned input proves it).
    let host = Arc::new(RecordingRuntimeHost::new(b"{\"input\":\"WRONG-PATH\"}"));
    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let run = runtime.block_on(async {
        let pre = executor
            .load_instance_pre(&compiled.wasm_path)
            .await
            .expect("load invoke-shaped artifact");
        executor
            .execute_invoke(
                &pre,
                runtara_component_host::WorkflowRunSpec {
                    env: HashMap::new(),
                    stderr: None,
                    timeout: Duration::from_secs(60),
                    cancel: None,
                    limits: runtara_component_host::WorkflowLimits::default(),
                    runtime: Some(host.clone()),
                },
                br#"{"input":"invoke-abi"}"#.to_vec(),
            )
            .await
    });

    let output = match run.exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("expected Completed, got {other:?}"),
    };
    let output_json: Value = serde_json::from_slice(&output).expect("output is JSON");
    assert_eq!(output_json, serde_json::json!({ "result": "invoke-abi" }));

    // runtime.complete still fires additively during the migration and must
    // carry the SAME bytes the return value carried.
    let recorded = host
        .completed
        .lock()
        .unwrap()
        .clone()
        .expect("complete fired additively");
    assert_eq!(recorded, output, "in-band and recorded outputs must agree");
}

#[test]
fn direct_wasm_execute_invoke_abi_returns_error_info_in_band() {
    let components_dir = direct_e2e_components_dir();
    let compiled =
        compile_invoke_abi_artifact(&components_dir, "invoke-abi-failed", ERROR_DIRECT_SIMPLE);

    let host = Arc::new(RecordingRuntimeHost::new(b"{}"));
    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let run = runtime.block_on(async {
        let pre = executor
            .load_instance_pre(&compiled.wasm_path)
            .await
            .expect("load invoke-shaped artifact");
        executor
            .execute_invoke(
                &pre,
                runtara_component_host::WorkflowRunSpec {
                    env: HashMap::new(),
                    stderr: None,
                    timeout: Duration::from_secs(60),
                    cancel: None,
                    limits: runtara_component_host::WorkflowLimits::default(),
                    runtime: Some(host.clone()),
                },
                br#"{"reason":"invoke-abi-error"}"#.to_vec(),
            )
            .await
    });

    let error = match run.exit {
        runtara_component_host::InvokeExit::Failed(error) => error,
        other => panic!("expected Failed, got {other:?}"),
    };
    // Structured decomposition: the fixture's error envelope maps
    // field-for-field into error-info (stdlib.invoke-error-fields).
    assert_eq!(error.code, "DIRECT_FAILURE");
    assert_eq!(error.message, "Direct workflow failure");
    assert_eq!(error.category, "permanent");
    assert_eq!(error.severity, "critical");
    assert!(!error.retryable);
    assert!(
        error
            .attributes
            .as_deref()
            .is_some_and(|attributes| attributes.contains("fixture")),
        "context attributes must survive: {:?}",
        error.attributes
    );

    // runtime.fail fired additively with the RAW envelope; the in-band
    // error is its structured decomposition — same payload, richer shape.
    let recorded = host
        .failed
        .lock()
        .unwrap()
        .clone()
        .expect("fail fired additively");
    let recorded_json: Value =
        serde_json::from_slice(&recorded).expect("recorded error is the JSON envelope");
    assert_eq!(recorded_json["code"], "DIRECT_FAILURE");
    assert_eq!(recorded_json["message"], error.message);
}

#[test]
fn direct_wasm_execute_invoke_abi_artifact_rejects_run_loader() {
    let components_dir = direct_e2e_components_dir();
    let compiled =
        compile_invoke_abi_artifact(&components_dir, "invoke-abi-shape", SIMPLE_PASSTHROUGH);

    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    // The legacy loader requires wasi:cli/run — an invoke-shaped artifact
    // must be rejected loudly, not executed as a no-op.
    match runtime.block_on(executor.load(&compiled.wasm_path)) {
        Ok(_) => panic!("wasi:cli/run loader must reject an invoke-shaped artifact"),
        Err(error) => assert!(
            format!("{error:#}").contains("wasi:cli/run"),
            "unexpected error: {error:#}"
        ),
    }
}

#[test]
fn direct_wasm_execute_invoke_abi_runs_durable_agent_step() {
    let components_dir = direct_e2e_components_dir();
    let compiled =
        compile_invoke_abi_artifact(&components_dir, "invoke-abi-agent", AGENT_CACHED_REPLAY);

    let host = Arc::new(RecordingRuntimeHost::new(b"{}"));
    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let run = runtime.block_on(async {
        let pre = executor
            .load_instance_pre(&compiled.wasm_path)
            .await
            .expect("load invoke-shaped agent artifact");
        executor
            .execute_invoke(
                &pre,
                runtara_component_host::WorkflowRunSpec {
                    env: HashMap::new(),
                    stderr: None,
                    timeout: Duration::from_secs(60),
                    cancel: None,
                    limits: runtara_component_host::WorkflowLimits::default(),
                    runtime: Some(host.clone()),
                },
                br#"{"value":"invoke-agent"}"#.to_vec(),
            )
            .await
    });

    // A durable agent step (utils return-input) composed under the invoke
    // world: agent imports + checkpoint host calls + the in-band result all
    // have to line up.
    let output = match run.exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("expected Completed, got {other:?}"),
    };
    let output_json: Value = serde_json::from_slice(&output).expect("output is JSON");
    assert_eq!(output_json, serde_json::json!({ "result": "invoke-agent" }));
}

/// Durable per-item delays inside a Split get PER-ITERATION sleep-checkpoint
/// keys (`{step}::{index}`) — without the loop-index fold every iteration
/// collides on one key, the hazard flagged (and deferred) by the unify plan.
/// Top-level durable delays keep the bare step id (asserted by the existing
/// delay tests' `checkpoint_id == "delay"` expectations).
#[test]
fn direct_wasm_execute_split_durable_delay_keys_are_per_iteration() {
    let components_dir = direct_e2e_components_dir();
    let graph = r#"{
      "name": "Split Durable Delay Keys",
      "durable": true,
      "steps": {
        "split": {
          "stepType": "Split",
          "id": "split",
          "name": "Per Item",
          "config": { "value": { "valueType": "reference", "value": "data.items" } },
          "subgraph": {
            "name": "Body",
            "entryPoint": "tick",
            "steps": {
              "tick": {
                "stepType": "Delay",
                "id": "tick",
                "name": "Tick",
                "durationMs": { "valueType": "immediate", "value": 1 }
              },
              "finish": {
                "stepType": "Finish",
                "id": "finish",
                "inputMapping": {
                  "v": { "valueType": "reference", "value": "item" }
                }
              }
            },
            "executionPlan": [ { "fromStep": "tick", "toStep": "finish" } ]
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
      "executionPlan": [ { "fromStep": "split", "toStep": "finish" } ],
      "variables": {},
      "inputSchema": { "items": { "type": "array", "required": true } },
      "outputSchema": {}
    }"#;

    let captured = run_direct_workflow_capture(
        &components_dir,
        "split-durable-delay-keys",
        graph,
        br#"{"items":[{"i":1},{"i":2}]}"#,
        false,
    );
    assert!(
        captured.status_success,
        "run failed: error={:?} stderr={}",
        captured.error_json, captured.stderr
    );
    let result = captured;

    let sleep_keys: Vec<&str> = result
        .sleeps
        .iter()
        .map(|sleep| sleep.checkpoint_id.as_str())
        .collect();
    assert_eq!(
        sleep_keys,
        vec!["tick::0", "tick::1"],
        "per-item durable delays must not collide on one sleep key"
    );
}

/// A single durable Delay whose only job downstream is to echo the input, so
/// the store-freeing (suspend/relaunch) and blocking (in-host sleep) lowerings
/// are trivially comparable at the output.
const STORE_FREEING_DELAY: &str = r#"{
  "name": "Store Freeing Delay",
  "durable": true,
  "steps": {
    "delay": {
      "stepType": "Delay",
      "id": "delay",
      "name": "Wait",
      "durationMs": { "valueType": "immediate", "value": 3600000 }
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "echo": { "valueType": "reference", "value": "data.value" }
      }
    }
  },
  "entryPoint": "delay",
  "executionPlan": [ { "fromStep": "delay", "toStep": "finish" } ],
  "variables": {},
  "inputSchema": { "value": { "type": "string", "required": true } },
  "outputSchema": {}
}"#;

/// Checkpoint-persisting runtime host: unlike [`RecordingRuntimeHost`] its
/// checkpoint map survives across `execute_invoke` calls (share one `Arc`), so
/// a store-freeing suspend that checkpoints its deadline on the first invoke
/// HITS on the second — the in-process stand-in for the wake scheduler
/// relaunching a parked instance. `sleeps` records blocking
/// `durable-sleep-checkpoint` calls (never fired on the store-freeing path).
struct CheckpointingRuntimeHost {
    input: Vec<u8>,
    checkpoints: Mutex<HashMap<String, Vec<u8>>>,
    completed: Mutex<Option<Vec<u8>>>,
    sleeps: Mutex<Vec<String>>,
    /// Externally-delivered custom signals keyed by checkpoint (signal) id —
    /// the wake-scheduler-side signal store. `poll_custom_signal` reads it
    /// non-destructively (a replayed wait re-reads the same signal).
    custom_signals: Mutex<HashMap<String, Vec<u8>>>,
    /// Fallback payload returned for ANY polled id — for the blocking control,
    /// whose deterministic signal id (workflow-id-scoped) isn't known ahead of
    /// the run.
    any_signal: Mutex<Option<Vec<u8>>>,
}

impl CheckpointingRuntimeHost {
    fn new(input: &[u8]) -> Self {
        Self {
            input: input.to_vec(),
            checkpoints: Mutex::new(HashMap::new()),
            completed: Mutex::new(None),
            sleeps: Mutex::new(Vec::new()),
            custom_signals: Mutex::new(HashMap::new()),
            any_signal: Mutex::new(None),
        }
    }

    fn deliver_signal(&self, checkpoint_id: &str, payload: &[u8]) {
        self.custom_signals
            .lock()
            .unwrap()
            .insert(checkpoint_id.to_string(), payload.to_vec());
    }

    fn deliver_signal_any(&self, payload: &[u8]) {
        *self.any_signal.lock().unwrap() = Some(payload.to_vec());
    }
}

#[async_trait::async_trait]
impl runtara_component_host::runtime_host::RuntimeHost for CheckpointingRuntimeHost {
    async fn load_input(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(Some(self.input.clone()))
    }
    fn instance_id(&self) -> Result<String, String> {
        Ok("store-freeing-delay".to_string())
    }
    async fn complete(&self, output: Vec<u8>) -> Result<(), String> {
        *self.completed.lock().unwrap() = Some(output);
        Ok(())
    }
    async fn fail(&self, _error: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
    async fn custom_event(&self, _kind: String, _payload: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
    fn debug_mode_enabled(&self) -> Result<bool, String> {
        Ok(false)
    }
    async fn breakpoint_pause(&self) -> Result<(), String> {
        Ok(())
    }
    async fn heartbeat(&self) -> Result<(), String> {
        Ok(())
    }
    async fn is_cancelled(&self) -> Result<bool, String> {
        Ok(false)
    }
    async fn check_signals(&self) -> Result<bool, String> {
        Ok(false)
    }
    async fn poll_custom_signal(&self, checkpoint_id: String) -> Result<Option<Vec<u8>>, String> {
        // Non-destructive read (mirrors the wait-replay fix): a resumed wait
        // re-reads the same delivered signal. Falls back to the any-id payload.
        if let Some(payload) = self.custom_signals.lock().unwrap().get(&checkpoint_id) {
            return Ok(Some(payload.clone()));
        }
        Ok(self.any_signal.lock().unwrap().clone())
    }
    async fn get_checkpoint(&self, checkpoint_id: String) -> Result<Option<Vec<u8>>, String> {
        Ok(self
            .checkpoints
            .lock()
            .unwrap()
            .get(&checkpoint_id)
            .cloned())
    }
    async fn checkpoint(
        &self,
        checkpoint_id: String,
        state: Vec<u8>,
    ) -> Result<runtara_component_host::runtime_host::RuntimeCheckpointResult, String> {
        // Mirror core `handle_checkpoint`: hit returns the stored state; a miss
        // saves only non-empty state (empty state is a read-only probe).
        let mut checkpoints = self.checkpoints.lock().unwrap();
        if let Some(existing) = checkpoints.get(&checkpoint_id) {
            return Ok(
                runtara_component_host::runtime_host::RuntimeCheckpointResult {
                    found: true,
                    state: existing.clone(),
                    pending_signal: None,
                    custom_signal: None,
                },
            );
        }
        if !state.is_empty() {
            checkpoints.insert(checkpoint_id, state);
        }
        Ok(
            runtara_component_host::runtime_host::RuntimeCheckpointResult {
                found: false,
                state: Vec::new(),
                pending_signal: None,
                custom_signal: None,
            },
        )
    }
    async fn handle_checkpoint_signal(&self, _signal_type: String) -> Result<bool, String> {
        Ok(false)
    }
    async fn record_retry_attempt(
        &self,
        _checkpoint_id: String,
        _attempt_number: u32,
        _error_message: Option<String>,
    ) -> Result<(), String> {
        Ok(())
    }
    async fn durable_sleep_checkpoint(
        &self,
        checkpoint_id: String,
        state: Vec<u8>,
        _ms: u64,
    ) -> Result<(), String> {
        // Blocking path: record the key (and persist the checkpoint like core's
        // handle_sleep) but never actually sleep — keeps the 1h fixture fast.
        if !state.is_empty() {
            self.checkpoints
                .lock()
                .unwrap()
                .entry(checkpoint_id.clone())
                .or_insert(state);
        }
        self.sleeps.lock().unwrap().push(checkpoint_id);
        Ok(())
    }
}

fn run_invoke_once(
    wasm_path: &Path,
    host: Arc<dyn runtara_component_host::runtime_host::RuntimeHost>,
    input: Vec<u8>,
) -> runtara_component_host::InvokeExit {
    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    runtime
        .block_on(async {
            let pre = executor
                .load_instance_pre(wasm_path)
                .await
                .expect("load invoke-shaped artifact");
            executor
                .execute_invoke(
                    &pre,
                    runtara_component_host::WorkflowRunSpec {
                        env: HashMap::new(),
                        stderr: None,
                        timeout: Duration::from_secs(60),
                        cancel: None,
                        limits: runtara_component_host::WorkflowLimits::default(),
                        runtime: Some(host),
                    },
                    input,
                )
                .await
        })
        .exit
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_millis() as u64
}

/// Store-freeing Slice 2: with the gate ON, a durable Delay under the invoke
/// export EXITS with `suspended(at(deadline))` on first reach (freeing the
/// Store) and, on relaunch, HITS the deadline checkpoint and skips the sleep —
/// completing with output byte-identical to the blocking lowering. This is the
/// in-process stand-in for the wake scheduler: the two invokes share one
/// checkpoint-persisting host, exactly as a relaunched instance shares the
/// durable checkpoint store.
#[test]
fn direct_wasm_execute_invoke_store_freeing_delay_suspends_then_resumes() {
    let components_dir = direct_e2e_components_dir();
    let input = br#"{"value":"resume-me"}"#.to_vec();
    let duration_ms = 3_600_000u64;

    // --- Store-freeing lowering (gate ON): suspend, then resume. ---
    let store_freeing = compile_invoke_abi_artifact_configured(
        &components_dir,
        "store-freeing-delay-on",
        STORE_FREEING_DELAY,
        true,
    );
    let host = Arc::new(CheckpointingRuntimeHost::new(&input));

    let before = now_ms();
    let first = run_invoke_once(&store_freeing.wasm_path, host.clone(), input.clone());
    let after = now_ms();

    let wakes = match first {
        runtara_component_host::InvokeExit::Suspended(wakes) => wakes,
        other => panic!("first invoke must suspend, got {other:?}"),
    };
    assert_eq!(wakes.len(), 1, "sequential lowering emits one wake");
    let deadline = match &wakes[0] {
        runtara_component_host::lifecycle::WorkflowWake::At(ms) => *ms,
        other => panic!("durable Delay must suspend on a timed wake, got {other:?}"),
    };
    // deadline == now_ms(at suspend) + duration, and the suspend happened
    // between `before` and `after`.
    assert!(
        deadline >= before + duration_ms && deadline <= after + duration_ms,
        "deadline {deadline} must be ~now+{duration_ms} (window {}..={})",
        before + duration_ms,
        after + duration_ms
    );
    // The deadline was persisted under the top-level delay key, and NO blocking
    // sleep fired.
    assert!(
        host.checkpoints.lock().unwrap().contains_key("delay"),
        "store-freeing suspend must checkpoint its deadline under the delay key"
    );
    assert!(
        host.sleeps.lock().unwrap().is_empty(),
        "store-freeing lowering must not call the blocking durable-sleep host fn"
    );
    assert!(
        host.completed.lock().unwrap().is_none(),
        "a suspended run has not completed yet"
    );

    // Relaunch: same host (checkpoint survives), replay from the start. The
    // deadline checkpoint HITS, the sleep is skipped, and the run completes.
    let second = run_invoke_once(&store_freeing.wasm_path, host.clone(), input.clone());
    let resumed_output = match second {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("relaunch must complete, got {other:?}"),
    };
    assert!(
        host.sleeps.lock().unwrap().is_empty(),
        "resume must not block either — the checkpoint HIT skips the sleep"
    );

    // --- Blocking lowering (gate OFF): completes in ONE invoke. ---
    let blocking = compile_invoke_abi_artifact_configured(
        &components_dir,
        "store-freeing-delay-off",
        STORE_FREEING_DELAY,
        false,
    );
    let blocking_host = Arc::new(CheckpointingRuntimeHost::new(&input));
    let blocking_exit = run_invoke_once(&blocking.wasm_path, blocking_host.clone(), input.clone());
    let blocking_output = match blocking_exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("blocking Delay must complete in one invoke, got {other:?}"),
    };
    // The blocking path DID call the durable-sleep host fn (its whole point),
    // proving the two lowerings diverge internally...
    assert_eq!(
        blocking_host.sleeps.lock().unwrap().as_slice(),
        &["delay".to_string()],
        "blocking lowering must go through durable-sleep-checkpoint"
    );

    // ...yet converge on byte-identical observable output. This is the
    // "semantics == legacy blocking, byte-preserved" guarantee.
    assert_eq!(
        resumed_output, blocking_output,
        "store-freeing resume output must byte-match the blocking output"
    );
    let expected: Value = serde_json::json!({ "echo": "resume-me" });
    assert_eq!(
        serde_json::from_slice::<Value>(&resumed_output).expect("output is JSON"),
        expected
    );
}

/// A bare WaitForSignal (no timeout) then a Finish echoing the signal payload.
const STORE_FREEING_WAIT: &str = r#"{
  "name": "Store Freeing Wait",
  "steps": {
    "wait": {
      "stepType": "WaitForSignal",
      "id": "wait",
      "name": "Approval",
      "pollIntervalMs": 0,
      "responseSchema": { "approved": { "type": "boolean", "required": true } }
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "approved": { "valueType": "reference", "value": "steps.wait.outputs.approved" }
      }
    }
  },
  "entryPoint": "wait",
  "executionPlan": [ { "fromStep": "wait", "toStep": "finish" } ],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}"#;

/// Store-freeing Slice 2 (on-signal waker half): with the gate ON, a durable
/// WaitForSignal under the invoke export EXITS with
/// `suspended(on-signal{signal-id, deadline})` on the first poll MISS (freeing
/// the Store) instead of blocking the poll loop. On relaunch — the in-process
/// stand-in for the custom-signal waker delivering the signal then the wake
/// scheduler relaunching — the wait re-polls the now-present signal and
/// completes. A no-timeout wait carries NO deadline (the waker is the sole wake
/// path); the blocking lowering (gate OFF) reaches the same output.
#[test]
fn direct_wasm_execute_invoke_store_freeing_wait_suspends_on_signal_then_resumes() {
    let components_dir = direct_e2e_components_dir();
    let input = br#"{}"#.to_vec();

    let artifact = compile_invoke_abi_artifact_configured(
        &components_dir,
        "store-freeing-wait-on",
        STORE_FREEING_WAIT,
        true,
    );
    let host = Arc::new(CheckpointingRuntimeHost::new(&input));

    // Invoke #1: the signal is absent, so the wait suspends on-signal.
    let first = run_invoke_once(&artifact.wasm_path, host.clone(), input.clone());
    let wakes = match first {
        runtara_component_host::InvokeExit::Suspended(wakes) => wakes,
        other => panic!("first invoke must suspend, got {other:?}"),
    };
    assert_eq!(wakes.len(), 1, "sequential lowering emits one wake");
    let (checkpoint_id, deadline) = match &wakes[0] {
        runtara_component_host::lifecycle::WorkflowWake::OnSignal(wait) => {
            (wait.checkpoint_id.clone(), wait.deadline_ms)
        }
        other => panic!("a WaitForSignal must suspend on-signal, got {other:?}"),
    };
    assert!(
        !checkpoint_id.is_empty(),
        "on-signal wake carries the deterministic wait signal id"
    );
    assert_eq!(
        deadline, None,
        "a no-timeout wait suspends without a deadline (waker is the sole wake path)"
    );
    assert!(
        host.sleeps.lock().unwrap().is_empty(),
        "store-freeing wait must not block on the poll interval"
    );
    assert!(host.completed.lock().unwrap().is_none());

    // Deliver the signal for the id the wake reported (the waker stand-in),
    // then relaunch (same host: the signal store + any checkpoints survive).
    host.deliver_signal(&checkpoint_id, br#"{"approved": true}"#);
    let second = run_invoke_once(&artifact.wasm_path, host.clone(), input.clone());
    let output = match second {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("relaunch after signal must complete, got {other:?}"),
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&output).expect("output is JSON"),
        serde_json::json!({ "approved": true }),
        "the resumed wait must surface the delivered signal payload"
    );

    // Control: the blocking lowering (gate OFF) reaches the same output when the
    // signal is already present (its poll loop finds it on the first pass).
    let blocking = compile_invoke_abi_artifact_configured(
        &components_dir,
        "store-freeing-wait-off",
        STORE_FREEING_WAIT,
        false,
    );
    let blocking_host = Arc::new(CheckpointingRuntimeHost::new(&input));
    // The blocking artifact's deterministic signal id is workflow-id-scoped and
    // differs from the store-freeing one, so pre-deliver for ANY polled id — its
    // first poll then finds the signal and the loop exits.
    blocking_host.deliver_signal_any(br#"{"approved": true}"#);
    let blocking_exit = run_invoke_once(&blocking.wasm_path, blocking_host.clone(), input.clone());
    let blocking_output = match blocking_exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("blocking wait with a present signal must complete, got {other:?}"),
    };
    assert_eq!(
        blocking_output, output,
        "store-freeing wait output must byte-match the blocking output"
    );
}
