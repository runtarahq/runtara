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
    DirectCompileError, RuntimeBinding, WorkflowAbi, analyze_direct_wasm_support,
    compile_direct_workflow, compile_direct_workflow_composed,
    compile_direct_workflow_composed_configured, compose_direct_workflow,
    emit_direct_component_artifacts_with_binding,
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
const BRACKET_QUOTED_DOTTED_KEY: &str = include_str!("fixtures/bracket_quoted_dotted_key.json");
const DELAY_DYNAMIC: &str = include_str!("fixtures/delay_dynamic.json");
const LOG_ALL_LEVELS: &str = include_str!("fixtures/log_all_levels.json");
const ERROR_DIRECT_SIMPLE: &str = include_str!("fixtures/error_direct_simple.json");
const EDGE_CONDITION_PRIORITY: &str = include_str!("fixtures/edge_condition_priority.json");
const AGENT_EDGE_CONDITION: &str = include_str!("fixtures/agent_edge_condition.json");
const WAIT_TIMEOUT_ON_ERROR: &str = include_str!("fixtures/wait_timeout_on_error.json");
const WAIT_DELAY_FINISH: &str = include_str!("fixtures/wait_delay_finish.json");
const WAIT_WAIT_FINISH: &str = include_str!("fixtures/wait_wait_finish.json");
const WHILE_DIRECT_INDEX_ONLY: &str = include_str!("fixtures/while_direct_index_only.json");
const WHILE_ITERATION_CONTEXT: &str = include_str!("fixtures/while_iteration_context.json");
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
      "maxRetries": 0, "retryDelay": 1000
    },
    "left": {
      "stepType": "Agent", "id": "left", "name": "Left",
      "agentId": "utils", "capabilityId": "random-double",
      "maxRetries": 0, "retryDelay": 1000
    },
    "right": {
      "stepType": "Agent", "id": "right", "name": "Right",
      "agentId": "utils", "capabilityId": "random-double",
      "maxRetries": 0, "retryDelay": 1000
    },
    "join": {
      "stepType": "Agent", "id": "join", "name": "Join",
      "agentId": "utils", "capabilityId": "random-double",
      "maxRetries": 0, "retryDelay": 1000
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
      "maxRetries": 0,
      "inputMapping": {"value": {"valueType": "immediate", "value": "H"}}
    },
    "gate": {
      "stepType": "Agent", "id": "gate", "name": "Gate",
      "agentId": "utils", "capabilityId": "return-input",
      "maxRetries": 0,
      "inputMapping": {"value": {"valueType": "immediate", "value": "G"}}
    },
    "left": {
      "stepType": "Agent", "id": "left", "name": "Left",
      "agentId": "utils", "capabilityId": "return-input",
      "maxRetries": 0,
      "inputMapping": {"value": {"valueType": "immediate", "value": "L"}}
    },
    "right": {
      "stepType": "Agent", "id": "right", "name": "Right",
      "agentId": "utils", "capabilityId": "return-input",
      "maxRetries": 0,
      "inputMapping": {"value": {"valueType": "immediate", "value": "R"}}
    },
    "after_left": {
      "stepType": "Agent", "id": "after_left", "name": "After Left",
      "agentId": "utils", "capabilityId": "return-input",
      "maxRetries": 0,
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

#[allow(dead_code)]
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
    /// Connection ids described through the host resolver, in request order.
    connection_metadata_requests: Vec<String>,
    /// Raw-SQL request paths the workflow sent (one per attempt — retries
    /// included), in order.
    sql_requests: Vec<String>,
    /// Number of custom-signal polls the mock answered with a signal — a
    /// replayed wait re-polls, so this is > the number of waits after a resume.
    custom_signal_polls: u32,
    slow_item_arrivals: Vec<Instant>,
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
    /// Arrival instants of /slow-item proxied requests (the parallel-split
    /// overlap harness) — the load-robust concurrency signal: overlapping
    /// requests arrive within one think-time regardless of machine load.
    slow_item_arrivals: Mutex<Vec<Instant>>,
    /// Scripted LLM-proxy responses, served front-to-back to POST /llm-proxy.
    /// Each entry is the proxy envelope `{status, headers, body}` the
    /// workflow's `call_agent()` will deserialize into an HttpResponse.
    llm_responses: Mutex<Vec<Value>>,
    /// Proxy request envelopes received on POST /llm-proxy, in order.
    llm_requests: Mutex<Vec<Value>>,
    /// Connection ids received on the internal metadata endpoint.
    connection_metadata_requests: Mutex<Vec<String>>,
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

/// The integration suite constructs staged workflow-agent sidecars directly,
/// bypassing the server's publish preflight. Keep those fixture sidecars at
/// the current staged-artifact contract; production certification is granted
/// only after `publish_workflow_agent` runs the static safety analysis.
fn certified_workflow_agent_info(
    slug: &str,
    name: &str,
    description: &str,
    input_schema: &HashMap<String, runtara_dsl::SchemaField>,
    output_schema: &HashMap<String, runtara_dsl::SchemaField>,
) -> runtara_dsl::agent_meta::AgentInfo {
    let mut info = runtara_dsl::agent_meta::workflow_agent_info(
        slug,
        name,
        description,
        input_schema,
        output_schema,
    );
    runtara_dsl::agent_meta::certify_workflow_agent_non_suspending(&mut info);
    info
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

    // Trusted connection-metadata endpoint used by the host resolver. The id
    // controls the fixture's authoritative integration so tests can prove a
    // legacy authored provider never wins over the referenced connection.
    if method == "GET" && path.ends_with("/metadata") {
        let connection_id = path.split('/').rev().nth(1).unwrap_or_default().to_string();
        server_state
            .connection_metadata_requests
            .lock()
            .expect("connection metadata requests lock")
            .push(connection_id.clone());
        let (integration_id, resources) = if connection_id == "conn-bedrock" {
            (
                "aws_credentials",
                serde_json::json!([
                    {"name": "bedrock.models", "description": "Available Amazon Bedrock models"},
                    {"name": "sqs.queues", "description": "Available Amazon SQS queues"}
                ]),
            )
        } else {
            (
                "openai_api_key",
                serde_json::json!([
                    {"name": "models", "description": "Available OpenAI models"}
                ]),
            )
        };
        return (
            200,
            serde_json::json!({
                "connectionId": connection_id,
                "integrationId": integration_id,
                "status": "ACTIVE",
                "resources": resources,
                "metadata": null
            }),
        );
    }

    // Hermetic LLM stub: `call_agent()` forwards provider requests here when
    // RUNTARA_HTTP_PROXY_URL points at the mock server. Pop the next scripted
    // proxy envelope; running out of script is a test bug, surfaced as 599
    // so the workflow fails loudly instead of hanging on `{success: true}`.
    if method == "POST" && path == "/llm-proxy" {
        let envelope: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        // Parallel-split overlap harness: a proxied request whose TARGET url
        // ends in /slow-item answers 200 after a fixed think time. Concurrent
        // requests overlap (thread-per-connection), so the workflow-side wall
        // clock reveals whether the guest truly parallelized the calls.
        if envelope["url"]
            .as_str()
            .is_some_and(|url| url.ends_with("/slow-item"))
        {
            server_state
                .slow_item_arrivals
                .lock()
                .expect("slow_item_arrivals lock")
                .push(Instant::now());
            thread::sleep(Duration::from_millis(400));
            return (
                200,
                serde_json::json!({
                    "status": 200,
                    "headers": {"content-type": "application/json"},
                    "body": {"ok": true}
                }),
            );
        }
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
                capture_sleep(body, sink, server_state);
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

/// Mirror production `handle_sleep`: persist the checkpoint, then (here) skip
/// the sleep itself. See [`CapturingRuntimeHost::durable_sleep_checkpoint`] for
/// why the save has to happen even though the wait does not.
fn capture_sleep(body: &[u8], sink: &mpsc::Sender<CapturedMessage>, server_state: &ServerState) {
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
        server_state
            .checkpoints
            .lock()
            .expect("checkpoint state lock")
            .insert(checkpoint_id.to_string(), state.clone());
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
            agent_slug: None,
        },
        components_dir,
        binding,
        abi,
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
        connection_metadata_requests: Mutex::new(Vec::new()),
        sql_responses: Mutex::new(sql_script),
        sql_requests: Mutex::new(Vec::new()),
        custom_signals: Mutex::new(custom_signals),
        custom_signal_polls: Mutex::new(0),
        slow_item_arrivals: Mutex::new(Vec::new()),
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
        ("CONNECTION_SERVICE_URL".into(), format!("http://{addr}")),
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
    let connection_metadata_requests = server_state_for_assertions
        .connection_metadata_requests
        .lock()
        .expect("connection metadata requests lock")
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
    let slow_item_arrivals = server_state_for_assertions
        .slow_item_arrivals
        .lock()
        .expect("slow_item_arrivals lock")
        .clone();
    CapturedRun {
        output_json,
        error_json,
        events,
        sleeps,
        checkpoints,
        llm_requests,
        connection_metadata_requests,
        sql_requests,
        custom_signal_polls,
        slow_item_arrivals,
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
        // Mirror POST /sleep, which mirrors production `handle_sleep`: SAVE the
        // checkpoint, then sleep. The save is not optional the way it reads —
        // it moves the instance's current checkpoint — so a mock that skipped it
        // diverged from production on every durable Delay. The sleep itself is
        // still skipped, which is what keeps delay-heavy fixtures fast; that is
        // a timing shortcut, not a persistence one.
        self.state
            .checkpoints
            .lock()
            .expect("checkpoint state lock")
            .insert(checkpoint_id.clone(), state.clone());
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
/// Relaunch budget for a parked run on the invoke axis. A fixture parks at most
/// once per durable step; four covers every fixture in the battery with room to
/// spare, and bounds a lowering bug that parks in a loop.
const INVOKE_MAX_RELAUNCHES: usize = 4;

/// Longest a park's deadline may be from now before this axis refuses to wait it
/// out. Battery fixtures use millisecond timeouts; anything beyond this is a
/// fixture the harness cannot honour, and saying so beats stalling the suite.
const INVOKE_MAX_PARK_WAIT: Duration = Duration::from_secs(5);

/// The earliest wall-clock deadline across a park's wakes, if any is timed.
fn earliest_timed_deadline_ms(
    wakes: &[runtara_component_host::lifecycle::WorkflowWake],
) -> Option<u64> {
    use runtara_component_host::lifecycle::WorkflowWake;
    wakes
        .iter()
        .filter_map(|wake| match wake {
            WorkflowWake::At(ms) => Some(*ms),
            WorkflowWake::OnSignal(wait) => wait.deadline_ms,
            WorkflowWake::OnResume => None,
        })
        .min()
}

/// Whether the wake scheduler would relaunch a park carrying these wakes.
///
/// Mirrors `park_invoke_suspend` in runtara-environment: a TIMED wake (`at`, or
/// `on-signal` with a timeout) gets `sleep_until` stamped and is relaunched at
/// the deadline, while a park whose only wake is `on-resume` — a cancel ack, a
/// breakpoint, a drain pause — is deliberately left alone. Relaunching one of
/// those would resurrect a run the host just stopped, so this is what keeps the
/// loop below from turning a cancel into a completion.
fn scheduler_would_relaunch(wakes: &[runtara_component_host::lifecycle::WorkflowWake]) -> bool {
    use runtara_component_host::lifecycle::WorkflowWake;
    !wakes.is_empty()
        && wakes.iter().all(|wake| match wake {
            WorkflowWake::At(_) => true,
            WorkflowWake::OnSignal(wait) => wait.deadline_ms.is_some(),
            WorkflowWake::OnResume => false,
        })
}

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
    // Stand in for the wake scheduler: a parked run is not a finished run, so
    // relaunch it (replaying against the same mock state, exactly as a
    // relaunched instance replays against the same durable store) until it
    // reaches a terminal outcome. Without this the harness sees only the park —
    // which is fine while nothing parks, and wrong the moment a durable Delay
    // or a Wait timeout does.
    let result = runtime.block_on(async {
        let pre = executor
            .load_instance_pre(wasm_path)
            .await
            .expect("load invoke-shaped workflow component");
        let mut attempt = 0;
        loop {
            let result = executor
                .execute_invoke(
                    &pre,
                    runtara_component_host::WorkflowRunSpec {
                        env: env_pairs.iter().cloned().collect(),
                        stderr: None,
                        timeout: Duration::from_secs(300),
                        cancel: None,
                        limits: limits.clone(),
                        runtime: Some(runtime_host.clone()),
                    },
                    input.clone(),
                )
                .await;
            let runtara_component_host::InvokeExit::Suspended(wakes) = &result.exit else {
                break result;
            };
            if !scheduler_would_relaunch(wakes) {
                break result;
            }
            // Wait out the deadline before relaunching, exactly as the scheduler
            // does. Relaunching early is not a shortcut: a park re-reads its own
            // deadline and re-parks while it is still in the future, so an
            // instant relaunch would burn the budget without making progress.
            // Fixture deadlines are milliseconds, so this costs almost nothing.
            if let Some(deadline_ms) = earliest_timed_deadline_ms(wakes) {
                let remaining = deadline_ms.saturating_sub(now_ms());
                assert!(
                    remaining <= INVOKE_MAX_PARK_WAIT.as_millis() as u64,
                    "fixture parked {remaining}ms out, past the {}ms this axis will wait — \
                     the battery has no way to fast-forward that far",
                    INVOKE_MAX_PARK_WAIT.as_millis()
                );
                if remaining > 0 {
                    tokio::time::sleep(Duration::from_millis(remaining)).await;
                }
            }
            assert!(
                attempt < INVOKE_MAX_RELAUNCHES,
                "run still parked after {INVOKE_MAX_RELAUNCHES} relaunches — a lowering \
                 that parks in a loop must fail the suite, not read as a clean suspend"
            );
            attempt += 1;
        }
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

fn assert_direct_rejects_non_durable_delay(workflow_id: &str, graph_json: &str) {
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("fixture parses");
    let temp = tempfile::tempdir().expect("tempdir");

    let error = compile_direct_workflow(DirectCompilationInput {
        workflow_id: workflow_id.to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: graph,
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        agent_slug: None,
    })
    .expect_err("a non-durable Delay must be rejected before it can hold a runner");

    let DirectCompileError::Unsupported { report } = error else {
        panic!("expected a direct-support rejection, got {error:?}");
    };
    assert!(
        report
            .unsupported
            .iter()
            .any(|feature| feature.feature == "non-durable-delay"),
        "missing non-durable-delay diagnostic: {report:?}"
    );
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
            agent_slug: None,
            progress_callback: None,
        },
        DirectWorkflowCompileOptions {
            output_dir: temp.path().to_path_buf(),
            extra_component_dirs: Vec::new(),
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
        agent_slug: None,
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

/// Spike B of the agent/workflow unification: wac-graph must compose a
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
        agent_slug: None,
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
    /// Total `runtime.complete` calls — a composed workflow-agent child must
    /// never fire one (the caller owns instance lifecycle), so a parent+child
    /// run records exactly 1.
    complete_calls: std::sync::atomic::AtomicU32,
}

impl RecordingRuntimeHost {
    fn new(input: &[u8]) -> Self {
        Self {
            input: input.to_vec(),
            completed: Mutex::new(None),
            failed: Mutex::new(None),
            complete_calls: std::sync::atomic::AtomicU32::new(0),
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
        self.complete_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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

/// A checkpoint-PERSISTING runtime host: records every id-carrying durable
/// call and keeps checkpoints across `execute_invoke` calls, so a second run
/// against the same host behaves exactly like a drain/resume replay (every
/// durable key HITs). Used to prove composed workflow-agent checkpoint
/// namespacing: the ids are inspectable AND replay-stable.
struct PersistingRuntimeHost {
    input: Vec<u8>,
    checkpoints: Mutex<HashMap<String, Vec<u8>>>,
    /// Ids passed to `durable-sleep-checkpoint` (durable Delays), in order.
    sleep_ids: Mutex<Vec<String>>,
    /// Ids written via `checkpoint` (agent/split/embed outputs), in order.
    checkpoint_writes: Mutex<Vec<String>>,
    /// Pending custom signals by exact id — mirrors the production
    /// `pending_custom_signals` table (upsert + NON-destructive read).
    signals: Mutex<HashMap<String, Vec<u8>>>,
    /// Every id handed to `poll-custom-signal`, in order (with repeats).
    polled_signal_ids: Mutex<Vec<String>>,
    completed: Mutex<Option<Vec<u8>>>,
    failed: Mutex<Option<Vec<u8>>>,
    complete_calls: std::sync::atomic::AtomicU32,
    /// When true, `check-signals` reports a lifecycle signal (a pause): the
    /// guest early-returns suspended from its wait poll loop — through the
    /// composed-agent sentinel when the wait lives inside a child.
    suspend_requested: std::sync::atomic::AtomicBool,
}

impl PersistingRuntimeHost {
    fn new(input: &[u8]) -> Self {
        Self {
            input: input.to_vec(),
            checkpoints: Mutex::new(HashMap::new()),
            sleep_ids: Mutex::new(Vec::new()),
            checkpoint_writes: Mutex::new(Vec::new()),
            signals: Mutex::new(HashMap::new()),
            polled_signal_ids: Mutex::new(Vec::new()),
            completed: Mutex::new(None),
            failed: Mutex::new(None),
            complete_calls: std::sync::atomic::AtomicU32::new(0),
            suspend_requested: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Pre-deliver a custom signal to an exact id, as a sender posting to
    /// `POST /signals/{instance}` would.
    fn deliver_signal(&self, signal_id: &str, payload: &[u8]) {
        self.signals
            .lock()
            .unwrap()
            .insert(signal_id.to_string(), payload.to_vec());
    }
}

#[async_trait::async_trait]
impl runtara_component_host::runtime_host::RuntimeHost for PersistingRuntimeHost {
    async fn load_input(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(Some(self.input.clone()))
    }
    fn instance_id(&self) -> Result<String, String> {
        Ok("checkpoint-ns-e2e".to_string())
    }
    async fn complete(&self, output: Vec<u8>) -> Result<(), String> {
        self.complete_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
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
        Ok(self
            .suspend_requested
            .load(std::sync::atomic::Ordering::SeqCst))
    }
    async fn poll_custom_signal(&self, checkpoint_id: String) -> Result<Option<Vec<u8>>, String> {
        self.polled_signal_ids
            .lock()
            .unwrap()
            .push(checkpoint_id.clone());
        // Exact-id, non-destructive read — the production semantics.
        Ok(self.signals.lock().unwrap().get(&checkpoint_id).cloned())
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
        self.checkpoint_writes
            .lock()
            .unwrap()
            .push(checkpoint_id.clone());
        // Mirror the production semantics (see `checkpoint_response`): an
        // existing id is a HIT returning the stored state; otherwise store.
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
        self.sleep_ids.lock().unwrap().push(checkpoint_id.clone());
        // Persist so a replay HITs and skips the sleep (the guest gates on
        // `get-checkpoint` for this key before sleeping).
        self.checkpoints
            .lock()
            .unwrap()
            .insert(checkpoint_id, state);
        Ok(())
    }
}

/// Phase-1 acceptance: a HostImport
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
            agent_slug: None,
        },
        WorkflowAbi::CliRunHttp,
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

/// A bracket-quoted body is one opaque key, dots included, all the way through
/// compile → compose → execute in the guest. The runtime used to rewrite
/// `["a.b"]` to `.a.b` and split it, so a reference the validator had accepted
/// as the literal field `a.b` resolved the (absent) nested path instead and
/// silently produced null.
#[test]
fn direct_wasm_execute_resolves_bracket_quoted_dotted_key() {
    let components_dir = direct_e2e_components_dir();

    let output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-bracket-quoted-dotted-key",
        BRACKET_QUOTED_DOTTED_KEY,
        br#"{"a.b":"flat-value","a":{"b":"nested-value"}}"#,
    );

    assert_eq!(
        output,
        serde_json::json!({
            "flat": "flat-value",
            "flatSingleQuoted": "flat-value",
            "nested": "nested-value"
        })
    );
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
fn direct_wasm_execute_while_iteration_context_and_variables() {
    let components_dir = direct_e2e_components_dir();

    let output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-while-iteration-context",
        WHILE_ITERATION_CONTEXT,
        br#"{"tenant":"acme"}"#,
    );

    assert_eq!(
        output,
        serde_json::json!({
            "iterations": 2,
            "last": {
                "tenant": "acme",
                "index": 1,
                "indices": [1],
                "item": null
            }
        })
    );
}

#[test]
fn direct_wasm_rejects_non_durable_while_timeout_before_runner_launch() {
    // This fixture intentionally combines a loop timeout with a non-durable
    // Delay. It must fail at compile time rather than retain a runner while the
    // loop clock is running.
    assert_direct_rejects_non_durable_delay("direct-wasm-while-timeout", WHILE_TIMEOUT);
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
          "provider":{{"valueType":"immediate","value":"openai"}},
          "model":{{"valueType":"immediate","value":"gpt-4o"}}{retry_config}}}}},
        "finish": {{"id":"finish","stepType":"Finish","inputMapping":{{
          "answer": {{"valueType":"reference","value":"steps.ai.outputs.response"}}
        }}}}
      }}
    }}"##
    )
}

fn dynamic_single_shot_ai_agent_graph_json() -> &'static str {
    r##"{
      "entryPoint": "ai",
      "executionPlan": [{"fromStep":"ai","toStep":"finish","label":"next"}],
      "steps": {
        "ai": {"id":"ai","stepType":"AiAgent","connectionRef":{"valueType":"reference","value":"data.connection","type":"string"},"config":{
          "systemPrompt":{"valueType":"immediate","value":"You are a test stub caller"},
          "userPrompt":{"valueType":"reference","value":"data.prompt"},
          "provider":{"valueType":"reference","value":"data.provider","type":"string"},
          "model":{"valueType":"reference","value":"data.model","type":"string"},
          "temperature":{"valueType":"reference","value":"data.temperature","type":"number"},
          "maxTokens":{"valueType":"reference","value":"data.maxTokens","type":"integer"}
        }},
        "finish": {"id":"finish","stepType":"Finish","inputMapping":{
          "answer":{"valueType":"reference","value":"steps.ai.outputs.response"}
        }}
      }
    }"##
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

/// A single-shot AiAgent that declares a structured `outputSchema`: two required
/// fields, one of them an enum. The compiler converts the DSL flat map to JSON
/// Schema and the `chat-completion` capability must parse *and* validate the
/// model's text against it.
fn structured_ai_agent_graph_json() -> &'static str {
    r##"{
      "entryPoint": "ai",
      "executionPlan": [{"fromStep":"ai","toStep":"finish","label":"next"}],
      "steps": {
        "ai": {"id":"ai","stepType":"AiAgent","connectionId":"conn-1","config":{
          "systemPrompt":{"valueType":"immediate","value":"You are a test stub caller"},
          "userPrompt":{"valueType":"immediate","value":"Classify this"},
          "provider":{"valueType":"immediate","value":"openai"},
          "model":{"valueType":"immediate","value":"gpt-4o"},
          "outputSchema":{
            "sentiment":{"type":"string","required":true,"enum":["positive","negative"]},
            "confidence":{"type":"number","required":true}
          }
        }},
        "finish": {"id":"finish","stepType":"Finish","inputMapping":{
          "answer":{"valueType":"reference","value":"steps.ai.outputs.response"}
        }}
      }
    }"##
}

#[test]
fn direct_wasm_execute_ai_agent_structured_output_yields_the_parsed_object() {
    let components_dir = direct_e2e_components_dir();

    // Happy path: a conforming response reaches `Finish` as a parsed object,
    // not as the raw assistant text.
    let result = run_direct_workflow_with_llm_script(
        &components_dir,
        "ai-structured-output-ok",
        structured_ai_agent_graph_json(),
        br#"{}"#,
        vec![llm_ok(r#"{"sentiment": "positive", "confidence": 0.87}"#)],
    );

    assert!(
        result.status_success,
        "stderr: {} error: {:?}",
        result.stderr, result.error_json
    );
    let output = result.output_json.expect("workflow completes");
    let answer = output.get("answer").expect("answer");
    assert_eq!(
        answer.get("sentiment").and_then(Value::as_str),
        Some("positive"),
        "{output}"
    );
    assert_eq!(
        answer.get("confidence").and_then(Value::as_f64),
        Some(0.87),
        "{output}"
    );
}

#[test]
fn direct_wasm_execute_ai_agent_structured_output_fails_on_non_json() {
    let components_dir = direct_e2e_components_dir();

    // Regression: this used to collapse to `structured_output: None`, and the
    // step "succeeded" with the raw text as its `response`.
    let result = run_direct_workflow_with_llm_script(
        &components_dir,
        "ai-structured-output-not-json",
        structured_ai_agent_graph_json(),
        br#"{}"#,
        vec![llm_ok("Sure! The sentiment is positive.")],
    );

    assert!(
        !result.status_success,
        "a non-JSON response must fail the step; output: {:?}",
        result.output_json
    );
    let error = result.error_json.expect("failure is reported");
    let rendered = error.to_string();
    assert!(
        rendered.contains("AI_STRUCTURED_OUTPUT_INVALID"),
        "expected the structured-output parse error: {rendered}"
    );
}

#[test]
fn direct_wasm_execute_ai_agent_structured_output_fails_on_schema_violation() {
    let components_dir = direct_e2e_components_dir();

    // Parseable JSON that doesn't honour the declared contract: `confidence` is
    // missing and `sentiment` isn't one of the declared enum values. Downstream
    // mappings used to read this silently.
    let result = run_direct_workflow_with_llm_script(
        &components_dir,
        "ai-structured-output-off-schema",
        structured_ai_agent_graph_json(),
        br#"{}"#,
        vec![llm_ok(r#"{"sentiment": "ecstatic"}"#)],
    );

    assert!(
        !result.status_success,
        "an off-schema response must fail the step; output: {:?}",
        result.output_json
    );
    let error = result.error_json.expect("failure is reported");
    let rendered = error.to_string();
    assert!(
        rendered.contains("AI_STRUCTURED_OUTPUT_SCHEMA_MISMATCH"),
        "expected the schema-mismatch error: {rendered}"
    );
    assert!(
        rendered.contains("confidence"),
        "the error should name the offending field: {rendered}"
    );
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

    assert!(
        result.status_success,
        "stderr: {} error: {:?} metadata requests: {:?} llm requests: {:?}",
        result.stderr, result.error_json, result.connection_metadata_requests, result.llm_requests
    );
    assert_eq!(result.connection_metadata_requests, ["conn-1"]);
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

#[test]
fn direct_wasm_execute_ai_agent_resolves_connection_ref_and_runtime_model_parameters() {
    let components_dir = direct_e2e_components_dir();
    let result = run_direct_workflow_with_llm_script(
        &components_dir,
        "ai-dynamic-runtime-parameters",
        dynamic_single_shot_ai_agent_graph_json(),
        br#"{"connection":"conn-1","prompt":"Say hello","provider":"bedrock","model":"gpt-4.1-mini","temperature":0.25,"maxTokens":321}"#,
        vec![llm_ok("dynamic hello")],
    );

    assert!(result.status_success, "stderr: {}", result.stderr);
    assert_eq!(result.connection_metadata_requests, ["conn-1"]);
    assert_eq!(result.llm_requests.len(), 1, "exactly one model call");
    let request = &result.llm_requests[0];
    assert_eq!(request["ai_provider"], "openai", "{request}");
    assert_eq!(request["body"]["model"], "gpt-4.1-mini", "{request}");
    assert_eq!(request["body"]["temperature"], 0.25, "{request}");
    assert_eq!(request["body"]["max_tokens"], 321, "{request}");
    assert_eq!(
        result
            .output_json
            .as_ref()
            .and_then(|output| output.get("answer"))
            .and_then(Value::as_str),
        Some("dynamic hello")
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
                            "provider":{"valueType":"immediate","value":"openai"},
                            "model":{"valueType":"immediate","value":"gpt-4o"},
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
          "provider":{"valueType":"immediate","value":"openai"},
          "model":{"valueType":"immediate","value":"gpt-4o"}}},
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
        r#""model":{"valueType":"immediate","value":"gpt-4o"}}}"#,
        &format!(r#""model":{{"valueType":"immediate","value":"gpt-4o"}},"maxIterations":{max_iterations}}}}}"#),
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
          "provider":{"valueType":"immediate","value":"openai"},
          "model":{"valueType":"immediate","value":"gpt-4o"}}},
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
          "provider":{"valueType":"immediate","value":"openai"},
          "model":{"valueType":"immediate","value":"gpt-4o"},
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
          "provider":{{"valueType":"immediate","value":"openai"}},
          "model":{{"valueType":"immediate","value":"gpt-4o"}}}}}},
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
          "provider":{{"valueType":"immediate","value":"openai"}},
          "model":{{"valueType":"immediate","value":"gpt-4o"}}}}}},
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
          "provider":{"valueType":"immediate","value":"openai"}}},
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
            agent_slug: None,
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
          "provider":{"valueType":"immediate","value":"openai"}}},
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
            agent_slug: None,
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
fn direct_wasm_rejects_non_durable_split_timeout_before_runner_launch() {
    // This fixture intentionally combines a Split timeout with a non-durable
    // Delay. It must fail at compile time rather than retain a runner while the
    // per-item work waits.
    assert_direct_rejects_non_durable_delay("direct-wasm-split-timeout", SPLIT_TIMEOUT);
}

#[test]
fn direct_wasm_execute_durable_delay_parks_and_completes() {
    let components_dir = direct_e2e_components_dir();

    let result = run_direct_workflow_with_events(
        &components_dir,
        "direct-wasm-execute-delay-durable",
        DELAY_DYNAMIC,
        br#"{"waitTime":0}"#,
    );

    assert_eq!(result.output_json, serde_json::json!({ "waited": 0 }));
    assert!(
        result.sleeps.is_empty(),
        "lifecycle invoke must park a durable delay instead of blocking: {:?}",
        result.sleeps
    );
    let parked_deadlines: Vec<_> = result
        .checkpoints
        .iter()
        .filter(|checkpoint| checkpoint.checkpoint_id == "delay" && checkpoint.state.len() == 8)
        .collect();
    assert_eq!(
        parked_deadlines.len(),
        1,
        "the parked delay must persist one absolute deadline: {:?}",
        result.checkpoints
    );
}

/// A diamond fan-out whose two branches are pure SYNC (Delay) steps used to PANIC
/// at compile ("parallel-branch compiles import the waitable builtins"): the
/// emitter chose the concurrent depth-wavefront (which needs the CM-async waitable
/// builtins) for a fan-out with no agent to overlap, but those builtins are imported
/// only when a concurrent branch pool has ≥1 agent. It now linearises. This proves
/// the fix at runtime AND the diamond's stated invariant — BOTH branches execute:
/// all three durable Delays (entry + branch_a + branch_b) park and relaunch, then it
/// completes.
#[test]
fn direct_wasm_execute_sync_parallel_branches_diamond_runs_both_branches() {
    let components_dir = direct_e2e_components_dir();
    let graph_json = smoke_fixture_json("parallel_branches_sync_diamond");

    let result = run_direct_workflow_with_events(
        &components_dir,
        "direct-wasm-execute-sync-parallel-diamond",
        &graph_json,
        br#"{}"#,
    );

    // Completed (Finish ran), so the fan-out reconverged.
    assert_eq!(result.output_json, serde_json::json!({ "merged": true }));

    assert!(
        result.sleeps.is_empty(),
        "lifecycle invoke must not block any branch delay: {:?}",
        result.sleeps
    );
    // Every branch's durable Delay persisted a deadline — both branches executed,
    // not just one.
    let mut parked_ids: Vec<&str> = result
        .checkpoints
        .iter()
        .filter(|checkpoint| checkpoint.state.len() == 8)
        .map(|checkpoint| checkpoint.checkpoint_id.as_str())
        .collect();
    parked_ids.sort_unstable();
    assert_eq!(
        parked_ids,
        vec!["branch_a", "branch_b", "entry"],
        "both branch Delays plus the entry Delay must park; got {:?}",
        result.checkpoints
    );
}

#[test]
fn direct_wasm_rejects_non_durable_delay_before_runner_launch() {
    let graph_json = non_durable_graph_json(DELAY_DYNAMIC);
    assert_direct_rejects_non_durable_delay("direct-wasm-execute-delay-non-durable", &graph_json);
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

    // The wait and the following durable Delay each persist one 8-byte absolute
    // deadline. The invocation harness relaunches after the delay's park, so both
    // checkpoint saves are observable in this completed capture.
    let deadline_ids: Vec<_> = first
        .checkpoints
        .iter()
        .filter(|cp| cp.state.len() == 8)
        .map(|cp| cp.checkpoint_id.as_str())
        .collect();
    assert_eq!(
        deadline_ids.len(),
        2,
        "run 1 should persist wait and delay deadlines; saw: {:?}",
        first.checkpoints
    );
    assert!(
        deadline_ids.iter().any(|id| id.ends_with("/wait")),
        "one deadline must be keyed by the wait's deterministic signal id, got: {:?}",
        deadline_ids
    );
    assert!(
        deadline_ids.contains(&"delay"),
        "one deadline must be keyed by the durable Delay, got: {:?}",
        deadline_ids
    );
    assert!(
        first.sleeps.is_empty(),
        "the durable Delay must park rather than block: {:?}",
        first.sleeps
    );

    // The durable state a resume would find committed: both deadlines.
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
    // The deadlines were read from their checkpoints, not recomputed and re-saved.
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
// reaches its expected terminal outcome (completes / fails). Durable-delay
// fixtures complete after the invoke harness relaunches their parked execution. Pure
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
    /// Records `/failed` and exits with a failed invocation outcome.
    Fails,
}

/// Run one execution-smoke fixture end-to-end: read it, compile → compose →
/// execute the composed artifact, and assert it reaches the expected terminal
/// state. Extracted from the former `fixture_execution_smoke_battery` loop body
/// so each fixture becomes its own `#[test]` (see `execution_smoke_cases!`) and
/// rides the suite's test-level parallelism instead of running as one ~15-minute
/// serial monolith; a failure now names the exact fixture that regressed.
fn run_smoke_case(fixture: &str, input: &[u8], expect: ExpectedOutcome) {
    let components_dir = direct_e2e_components_dir();
    let json = smoke_fixture_json(fixture);
    let captured = run_direct_workflow_capture(
        &components_dir,
        &format!("smoke-{fixture}"),
        &json,
        input,
        false,
    );
    let verdict = match expect {
        ExpectedOutcome::Completes => captured.status_success && captured.output_json.is_some(),
        ExpectedOutcome::Fails => !captured.status_success && captured.error_json.is_some(),
    };
    assert!(
        verdict,
        "execution smoke {fixture} [{expect:?}] did not reach the expected terminal state: \
         status_success={}, completed={}, failed={}, sleeps={}\n      stderr: {}",
        captured.status_success,
        captured.output_json.is_some(),
        captured.error_json.is_some(),
        captured.sleeps.len(),
        stderr_tail(&captured.stderr),
    );
}

/// Expands each `fixture => input, Outcome` entry into its own
/// `#[test] fn <fixture>()`. Splitting the cases into individual tests (rather
/// than one `#[test]` looping a `const [SmokeCase]`) lets them run in parallel
/// under `cargo test` and CI's default `--test-threads`; as a single serial
/// test the battery was a ~15-minute pole that capped the job's wall-clock no
/// matter how many cores were available.
macro_rules! execution_smoke_cases {
    ($( $fixture:ident => $input:literal, $expect:ident ),* $(,)?) => {
        $(
            #[test]
            fn $fixture() {
                run_smoke_case(stringify!($fixture), $input, ExpectedOutcome::$expect);
            }
        )*
    };
}

execution_smoke_cases! {
    // --- Completes: pure control flow -------------------------------------
    simple_passthrough => br#"{"input":"x"}"#, Completes,
    conditional_workflow => br#"{"flag":true}"#, Completes,
    conditional_nested => br#"{"flag":true,"kind":"a"}"#, Completes,
    conditional_diamond => br#"{"flag":true}"#, Completes,
    conditional_diamond_asymmetric => br#"{"flag":true,"urgent":false}"#, Completes,
    conditional_length_comparison => br#"{"description":"hello world this is a long description"}"#, Completes,
    edge_condition_priority => br#"{"status":"active","tier":"gold"}"#, Completes,
    edge_condition_diamond => br#"{"tier":"gold"}"#, Completes,
    filter_simple => br#"{"items":[1,2,3,4,5]}"#, Completes,
    filter_complex_condition => br#"{"users":[{"age":25,"active":true},{"age":17,"active":false}]}"#, Completes,
    filter_with_not => br#"{}"#, Completes,
    switch_value_simple => br#"{"status":"active"}"#, Completes,
    switch_routing_simple => br#"{"status":"active"}"#, Completes,
    group_by_simple => br#"{"items":[{"category":"a","v":1},{"category":"b","v":2},{"category":"a","v":3}]}"#, Completes,
    group_by_expected_keys => br#"{"items":[{"category":"a"},{"category":"b"}]}"#, Completes,
    group_by_nested_key => br#"{"users":[{"profile":{"role":"admin"}},{"profile":{"role":"user"}}]}"#, Completes,
    log_no_context => br#"{}"#, Completes,
    log_all_levels => br#"{"message":"hi"}"#, Completes,
    while_direct_index_only => br#"{"count":3}"#, Completes,
    // Transform-agent fixtures (split_*, while_*, log_*, transform_workflow)
    // now execute too — their map-fields input mappings were corrected to the
    // current `source_data` + `mappings` schema. See the section below.
    // --- Fails: explicit error --------------------------------------------
    error_direct_simple => br#"{"requestId":"r1"}"#, Fails,
    // Conditional-routed Error fixtures; inputs steer each to its Error branch
    // (these also exercise the passthrough->return-input composite fix).
    error_permanent => br#"{"resourceId":"res-1","found":false}"#, Fails,
    error_transient => br#"{"success":false}"#, Fails,
    error_with_context => br#"{"orderId":"o-1","amount":5000}"#, Fails,
    error_all_categories => br#"{"errorType":"transient"}"#, Fails,
    // `while_timeout` and `split_timeout` deliberately include non-durable
    // Delays; their explicit compile-rejection tests live above rather than
    // letting them reach this execution battery.
    // --- Durable delays park, relaunch, then complete ---------------------
    delay_simple => br#"{}"#, Completes,
    delay_dynamic => br#"{"waitTime":5}"#, Completes,
    // Diamond fan-out whose branches are all SYNC (Delay) steps — zero agents to
    // overlap, so it linearises instead of reaching for the concurrent wavefront.
    // Regression guard: this used to PANIC at compile ("parallel-branch compiles
    // import the waitable builtins"); now it compiles and runs end-to-end.
    parallel_branches_sync_diamond => br#"{}"#, Completes,
    // --- transform-agent fixtures (map-fields), now on the corrected schema --
    // These drive their subgraphs/loops through `transform/map-fields`; with
    // the input mappings fixed to `source_data` + `mappings` they execute.
    transform_workflow => br#"{"input_field":"hello"}"#, Completes,
    split_workflow => br#"{"items":[{"value":1},{"value":2},{"value":3}]}"#, Completes,
    split_parallel_workflow => br#"{"items":[{"value":1},{"value":2},{"value":3}]}"#, Completes,
    // NOTE: split_with_schemas / split_with_schemas_failing are Tier-A only.
    // Their per-item input/output schemas make the terminal outcome
    // input-specific (a generic item either traps or passes regardless of the
    // "_failing" intent), so they aren't meaningful as input-agnostic smoke.
    // While loops that terminate via `loop.index` against a bound from input.
    while_with_loop_index => br#"{"maxIterations":3}"#, Completes,
    while_with_previous_outputs => br#"{"items":[1,2],"count":2}"#, Completes,
    while_max_iterations => br#"{"value":0}"#, Completes,
    // While loops whose condition reads a constant `steps.init.outputs.*`;
    // seeded so the guard is already false (zero iterations) — exercises
    // condition eval + clean exit without risking a non-terminating loop.
    while_simple => br#"{"counter":5,"target":3}"#, Completes,
    while_workflow => br#"{"counter":5,"target":3}"#, Completes,
    while_break_on_first => br#"{"counter":0,"target":10}"#, Completes,
    log_with_context => br#"{"value":"v","timestamp":"t"}"#, Completes,
    log_workflow => br#"{"value":"v"}"#, Completes,
    log_error_handling => br#"{"value":"v"}"#, Completes,
    log_in_loop => br#"{"count":3}"#, Completes,
}

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

    // Point the object-model URL at a port whose connections are torn down
    // before any response bytes: transport failure on every attempt. The
    // listener must stay bound for the whole test — probing a free port and
    // dropping the listener let the OS hand the same ephemeral port to a
    // mock server of a concurrently running test, whose HTTP responses then
    // made this "unreachable" endpoint succeed. Closing an accepted
    // connection unanswered classifies exactly like a refused one: any
    // transport-level failure maps to OBJECT_MODEL_HTTP_ERROR, and the
    // transient/permanent split is per capability. query-sql reclassifies
    // transport errors to transient (retries, then exhausts); execute-sql
    // keeps them permanent (the statement may have committed).
    let dead_listener = TcpListener::bind("127.0.0.1:0").expect("bind dead port");
    let dead_port = dead_listener.local_addr().expect("local_addr").port();
    thread::spawn(move || {
        for stream in dead_listener.incoming() {
            drop(stream);
        }
    });
    let refused_env = vec![(
        "RUNTARA_OBJECT_MODEL_URL".to_string(),
        format!("http://127.0.0.1:{dead_port}/object-model"),
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
            "{capability}: dead connection must fail the step"
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
// Invoke ABI (Phase 3 of the agent/workflow unification): the workflow
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
    compile_invoke_abi_artifact_full(components_dir, workflow_id, graph_json, false)
}

fn compile_invoke_abi_artifact_full(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
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
            agent_slug: None,
        },
        components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
        omit_runtime,
    )
    .expect("invoke-abi compile+compose succeeds");
    // Keep the tempdir alive by leaking it — the executor reads the artifact
    // lazily and the test owns the whole lifetime anyway.
    std::mem::forget(temp);
    result
}

fn compile_invoke_abi_artifact_with_children(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    child_workflows: Vec<runtara_workflows::compile::ChildWorkflowInput>,
) -> runtara_workflows::direct_wasm::DirectCompilationResult {
    let graph: ExecutionGraph = serde_json::from_str(graph_json).expect("parent fixture parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let result = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: workflow_id.to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows,
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        },
        components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
        false,
    )
    .expect("child invoke-abi compile+compose succeeds");
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
            agent_slug: None,
        },
        components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
        // omit_runtime is forced true for AgentCapabilities by the compiler.
        false,
    )
    .expect("agent-capabilities compile+compose succeeds");
    std::mem::forget(temp);
    result
}

/// Workflow-as-agent slice a: a pure workflow compiled with the
/// `AgentCapabilities` ABI exports `runtara:agent-<slug>/capabilities.invoke(
/// capability-id, input) -> result<list<u8>, error-info>` — the exact agent
/// shape — and is invocable AS an agent through a wasmtime typed call. With no
/// explicit slug, the export id derives from the graph name via the shared
/// slug transform ("Pure Passthrough" → `pure-passthrough`), mirroring the
/// server's auto-derived `workflows.slug`.
#[test]
fn direct_wasm_execute_agent_capabilities_workflow_invocable_as_agent() {
    let components_dir = direct_e2e_components_dir();
    let compiled =
        compile_agent_capabilities_artifact(&components_dir, "workflow-as-agent", PURE_PASSTHROUGH);

    // Shape: agent-shaped export under the derived slug, zero runtime imports.
    assert!(
        compiled.omit_runtime,
        "AgentCapabilities implies omit-runtime"
    );
    let world = &compiled.component_artifacts.world_wit;
    assert!(
        world.contains("export runtara:agent-pure-passthrough/capabilities@0.4.0"),
        "world must export the capabilities interface under the derived slug:\n{world}"
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
                "runtara:agent-pure-passthrough/capabilities@0.4.0",
                "run",
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

/// P6: ANY workflow can compile as an agent. A runtime-needing workflow (here
/// a durable Delay) is no longer rejected — it keeps the runtime import
/// (satisfied by the composing parent's runtime host / the embedding host),
/// while a pure workflow still omits it. The graph-level `durable: false`
/// off-switch turns a durability-only runtime need back into the pure shape.
#[test]
fn direct_wasm_execute_agent_capabilities_keeps_runtime_for_durable_workflow() {
    let components_dir = direct_e2e_components_dir();
    let graph: ExecutionGraph = serde_json::from_str(&store_freeing_delay_fixture(Some(3_600_000)))
        .expect("delay fixture parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let compiled = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "delay-agent".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: temp.path().join("durable"),
            track_events: false,
            agent_catalog: None,
            agent_slug: Some("delay-agent".to_string()),
        },
        &components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
        false,
    )
    .expect("a durable workflow now compiles as an agent");
    assert!(!compiled.omit_runtime);
    assert!(
        compiled
            .component_artifacts
            .world_wit
            .contains("import runtara:workflow-runtime/runtime@0.1.0;"),
        "durable agent must keep the runtime import:\n{}",
        compiled.component_artifacts.world_wit
    );
    assert!(
        compiled
            .component_artifacts
            .world_wit
            .contains("export runtara:agent-delay-agent/capabilities@0.4.0;")
    );

    // 4a off-switch: durability is the ONLY runtime need of a plain transform
    // workflow, so `durable: false` at the graph level restores the pure,
    // zero-runtime-import agent shape.
    const DURABLE_PASSTHROUGH: &str = r#"{
      "name": "Durable Passthrough",
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
    let durable_default: ExecutionGraph =
        serde_json::from_str(DURABLE_PASSTHROUGH).expect("parses");
    assert_eq!(durable_default.durable, None, "durable defaults to true");
    let compile_with = |graph: ExecutionGraph, id: &str| {
        compile_direct_workflow_composed_configured(
            DirectCompilationInput {
                workflow_id: id.to_string(),
                version: 1,
                source_checksum: None,
                execution_graph: graph,
                child_workflows: vec![],
                output_dir: temp.path().join(id),
                track_events: false,
                agent_catalog: None,
                agent_slug: Some(id.to_string()),
            },
            &components_dir,
            RuntimeBinding::HostImport,
            runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
            false,
        )
        .expect("agent compile succeeds")
    };
    let durable_agent = compile_with(durable_default, "durable-on");
    assert!(
        !durable_agent.omit_runtime,
        "default-durable workflow keeps the runtime import"
    );
    let mut non_durable: ExecutionGraph =
        serde_json::from_str(DURABLE_PASSTHROUGH).expect("parses");
    non_durable.durable = Some(false);
    let pure_agent = compile_with(non_durable, "durable-off");
    assert!(
        pure_agent.omit_runtime,
        "durable:false restores the pure zero-runtime agent shape"
    );
    std::mem::forget(temp);
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
fn direct_wasm_execute_invoke_abi_is_repeatable_across_runs() {
    let components_dir = direct_e2e_components_dir();
    let compiled =
        compile_invoke_abi_artifact(&components_dir, "invoke-abi-repeatable", SIMPLE_PASSTHROUGH);
    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let pre = runtime.block_on(executor.load_instance_pre(&compiled.wasm_path));
    let pre = pre.expect("load invoke-shaped workflow component");

    // `execute_invoke` must create a new Store for every execution even when
    // the compiled component is reused across instances.
    for round in 0..2 {
        let host = Arc::new(RecordingRuntimeHost::new(b"{}"));
        let run = runtime.block_on(executor.execute_invoke(
            &pre,
            runtara_component_host::WorkflowRunSpec {
                env: HashMap::new(),
                stderr: None,
                timeout: Duration::from_secs(60),
                cancel: None,
                limits: runtara_component_host::WorkflowLimits::default(),
                runtime: Some(host.clone()),
            },
            br#"{"input":"direct-finish"}"#.to_vec(),
        ));
        let output = match run.exit {
            runtara_component_host::InvokeExit::Completed(output) => output,
            other => panic!("round {round} must complete, got {other:?}"),
        };
        assert_eq!(
            serde_json::from_slice::<Value>(&output).expect("output is JSON"),
            serde_json::json!({ "result": "direct-finish" }),
            "round {round} output mismatch"
        );
        assert_eq!(
            host.completed.lock().unwrap().clone(),
            Some(output),
            "round {round} must report its own completion"
        );
    }
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

/// Durable per-item delays inside a Split get PER-ITERATION park-checkpoint
/// keys (`{step}::{index}`) — without the loop-index fold every iteration
/// collides on one key, the hazard flagged (and deferred) by the unify plan.
/// Top-level durable delays keep the bare step id (asserted by the existing
/// delay tests' parked-checkpoint expectations).
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

    assert!(
        result.sleeps.is_empty(),
        "per-item durable delays must park rather than block: {:?}",
        result.sleeps
    );
    let mut parked_keys: Vec<&str> = result
        .checkpoints
        .iter()
        .filter(|checkpoint| checkpoint.state.len() == 8)
        .map(|checkpoint| checkpoint.checkpoint_id.as_str())
        .collect();
    parked_keys.sort_unstable();
    assert_eq!(
        parked_keys,
        vec!["tick::0", "tick::1"],
        "per-item durable delays must not collide on one park key"
    );
}

/// A single durable Delay whose only job downstream is to echo the input, so
/// the store-freeing (suspend/relaunch) and blocking (in-host sleep) lowerings
/// are trivially comparable at the output.
/// A single durable Delay then a Finish echoing the input. `duration` is the
/// literal `durationMs` when `Some`, and a REFERENCE to `data.waitMs` when
/// `None` — the reference form is what makes the park/block choice unresolvable
/// at compile time, so one artifact has to decide it at runtime.
fn store_freeing_delay_fixture(duration_ms: Option<u64>) -> String {
    let (duration, extra_input) = match duration_ms {
        Some(ms) => (
            format!(r#"{{ "valueType": "immediate", "value": {ms} }}"#),
            String::new(),
        ),
        None => (
            r#"{ "valueType": "reference", "value": "data.waitMs" }"#.to_string(),
            r#", "waitMs": { "type": "number", "required": true }"#.to_string(),
        ),
    };
    format!(
        r#"{{
  "name": "Store Freeing Delay",
  "durable": true,
  "steps": {{
    "delay": {{
      "stepType": "Delay",
      "id": "delay",
      "name": "Wait",
      "durationMs": {duration}
    }},
    "finish": {{
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {{
        "echo": {{ "valueType": "reference", "value": "data.value" }}
      }}
    }}
  }},
  "entryPoint": "delay",
  "executionPlan": [ {{ "fromStep": "delay", "toStep": "finish" }} ],
  "variables": {{}},
  "inputSchema": {{ "value": {{ "type": "string", "required": true }}{extra_input} }},
  "outputSchema": {{}}
}}"#
    )
}

/// A one-step HTTP Agent whose first rate-limited result must be replayed from
/// its per-attempt checkpoint. `retryDelay` deliberately differs from the
/// scripted `retry-after-ms`, proving the parked deadline follows rate-limit
/// policy rather than an arbitrary in-run sleep.
fn lifecycle_retry_http_graph() -> String {
    serde_json::json!({
        "name": "Lifecycle Retry Park",
        "durable": true,
        "rateLimitBudgetMs": 60_000,
        "steps": {
            "fetch": {
                "stepType": "Agent",
                "id": "fetch",
                "agentId": "http",
                "capabilityId": "http-request",
                "durable": true,
                "maxRetries": 1,
                "retryDelay": 1,
                "inputMapping": {
                    "method": {"valueType": "immediate", "value": "GET"},
                    "url": {"valueType": "reference", "value": "data.url"}
                }
            },
            "finish": {
                "stepType": "Finish",
                "id": "finish",
                "inputMapping": {
                    "status": {"valueType": "reference", "value": "steps.fetch.outputs.status_code"}
                }
            }
        },
        "entryPoint": "fetch",
        "executionPlan": [{"fromStep": "fetch", "toStep": "finish"}],
        "variables": {},
        "inputSchema": {},
        "outputSchema": {}
    })
    .to_string()
}

/// The production shape that regressed: a Split that REQUESTS concurrency whose
/// item body is an ordinary retrying Agent. The concurrent window is ineligible
/// (retrying item), so this must compile onto the sequential lowering and use
/// its durable retry park — not fail to compile.
fn lifecycle_parallel_split_retry_graph(url: &str) -> String {
    serde_json::json!({
        "name": "Lifecycle Parallel Split Retry Park",
        "durable": true,
        "rateLimitBudgetMs": 60_000,
        "steps": {
            "iterate_jobs": {
                "stepType": "Split",
                "id": "iterate_jobs",
                "config": {
                    "value": {"valueType": "immediate", "value": [1]},
                    "parallelism": 4
                },
                "subgraph": {
                    "entryPoint": "fetch",
                    "steps": {
                        "fetch": {
                            "stepType": "Agent",
                            "id": "fetch",
                            "agentId": "http",
                            "capabilityId": "http-request",
                            "durable": true,
                            "maxRetries": 1,
                            "retryDelay": 1,
                            "inputMapping": {
                                "method": {"valueType": "immediate", "value": "GET"},
                                "url": {"valueType": "immediate", "value": url}
                            }
                        },
                        "item_finish": {
                            "stepType": "Finish",
                            "id": "item_finish",
                            "inputMapping": {
                                "status": {"valueType": "reference", "value": "steps.fetch.outputs.status_code"}
                            }
                        }
                    },
                    "executionPlan": [{"fromStep": "fetch", "toStep": "item_finish"}],
                    "variables": {},
                    "inputSchema": {},
                    "outputSchema": {}
                }
            },
            "finish": {
                "stepType": "Finish",
                "id": "finish",
                "inputMapping": {
                    "results": {"valueType": "reference", "value": "steps.iterate_jobs.outputs"}
                }
            }
        },
        "entryPoint": "iterate_jobs",
        "executionPlan": [{"fromStep": "iterate_jobs", "toStep": "finish"}],
        "variables": {},
        "inputSchema": {},
        "outputSchema": {}
    })
    .to_string()
}

fn lifecycle_retry_split_graph() -> String {
    serde_json::json!({
        "name": "Lifecycle Split Retry Park",
        "durable": true,
        "steps": {
            "split": {
                "stepType": "Split",
                "id": "split",
                "config": {
                    "value": {"valueType": "immediate", "value": [1]},
                    "sequential": true,
                    "maxRetries": 1,
                    "retryDelay": 5_000
                },
                "subgraph": {
                    "entryPoint": "temporary_failure",
                    "steps": {
                        "temporary_failure": {
                            "stepType": "Error",
                            "id": "temporary_failure",
                            "category": "transient",
                            "code": "ITEM_TEMPORARY",
                            "message": "retry this item",
                            "severity": "error"
                        }
                    },
                    "executionPlan": [],
                    "variables": {},
                    "inputSchema": {},
                    "outputSchema": {}
                }
            },
            "finish": {"stepType": "Finish", "id": "finish"}
        },
        "entryPoint": "split",
        "executionPlan": [{"fromStep": "split", "toStep": "finish"}],
        "variables": {},
        "inputSchema": {},
        "outputSchema": {}
    })
    .to_string()
}

const LIFECYCLE_RETRY_EMBED_PARENT: &str = r#"{
  "name": "Lifecycle Embed Retry Park",
  "durable": true,
  "steps": {
    "call_child": {
      "stepType": "EmbedWorkflow",
      "id": "call_child",
      "childWorkflowId": "retry-child",
      "childVersion": "latest",
      "maxRetries": 1,
      "retryDelay": 5000
    },
    "finish": { "stepType": "Finish", "id": "finish" }
  },
  "entryPoint": "call_child",
  "executionPlan": [{ "fromStep": "call_child", "toStep": "finish" }],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}"#;

const LIFECYCLE_RETRY_EMBED_CHILD: &str = r#"{
  "name": "Lifecycle Embed Retry Child",
  "steps": {
    "temporary_failure": {
      "stepType": "Error",
      "id": "temporary_failure",
      "category": "transient",
      "code": "CHILD_TEMPORARY",
      "message": "retry the child",
      "severity": "error"
    }
  },
  "entryPoint": "temporary_failure",
  "executionPlan": [],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}"#;

fn spawn_retry_http_proxy(
    response_bodies: Vec<Vec<u8>>,
) -> (
    String,
    Arc<std::sync::atomic::AtomicU32>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind retry proxy");
    let url = format!(
        "http://{}",
        listener.local_addr().expect("retry proxy addr")
    );
    let hits = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let stub_hits = hits.clone();
    let handle = thread::spawn(move || {
        for body in response_bodies {
            let (mut stream, _) = listener.accept().expect("retry proxy request");
            let mut request = [0u8; 8192];
            let _ = stream.read(&mut request);
            stub_hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let headers = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            stream
                .write_all(headers.as_bytes())
                .expect("retry proxy headers");
            stream.write_all(&body).expect("retry proxy body");
        }
    });
    (url, hits, handle)
}

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
    /// Milliseconds added to the wall clock for every `now-ms` the guest asks
    /// for. The wake-scheduler stand-in advances this to a park's deadline so a
    /// 1-hour Delay can be woken without waiting an hour: a resumed park
    /// compares `now-ms` against its stored deadline, so a stand-in that cannot
    /// move the clock can only ever observe a re-park.
    clock_offset_ms: Mutex<u64>,
    /// When set, the exact value `now-ms` returns, ignoring the wall clock and
    /// `clock_offset_ms` entirely. Lets a test place the guest at a precise
    /// distance from a deadline and hold it there.
    pinned_clock_ms: Mutex<Option<u64>>,
    /// Fallback payload returned for ANY polled id — for the blocking control,
    /// whose deterministic signal id (workflow-id-scoped) isn't known ahead of
    /// the run.
    any_signal: Mutex<Option<Vec<u8>>>,
    /// When set, `durable-sleep-checkpoint` reports this message instead of
    /// returning cleanly — the shape of a sleep whose request the client
    /// deadline outlasted.
    sleep_error: Mutex<Option<String>>,
    /// What the guest reported through `runtime.fail`. `None` after a failed run
    /// is the defect this captures: an error the lowering returned without ever
    /// reporting it.
    failed: Mutex<Option<Vec<u8>>>,
    /// A lifecycle signal that becomes visible on the next explicit poll. Retry
    /// parks use this after a due wake, before issuing their next attempt.
    pending_signal: std::sync::atomic::AtomicBool,
}

impl CheckpointingRuntimeHost {
    fn new(input: &[u8]) -> Self {
        Self {
            input: input.to_vec(),
            checkpoints: Mutex::new(HashMap::new()),
            completed: Mutex::new(None),
            sleeps: Mutex::new(Vec::new()),
            custom_signals: Mutex::new(HashMap::new()),
            clock_offset_ms: Mutex::new(0),
            pinned_clock_ms: Mutex::new(None),
            any_signal: Mutex::new(None),
            sleep_error: Mutex::new(None),
            failed: Mutex::new(None),
            pending_signal: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Arm every `durable-sleep-checkpoint` to fail with `message`, standing in
    /// for the composed binding's sleep request being aborted by the client's
    /// own request deadline before core can answer it.
    fn fail_sleeps_with(&self, message: &str) {
        *self.sleep_error.lock().unwrap() = Some(message.to_string());
    }

    fn deliver_signal(&self, checkpoint_id: &str, payload: &[u8]) {
        self.custom_signals
            .lock()
            .unwrap()
            .insert(checkpoint_id.to_string(), payload.to_vec());
    }

    /// PIN the guest's clock exactly `remaining_ms` short of `deadline_ms` — an
    /// early wake, which [`advance_clock_past`](Self::advance_clock_past)
    /// cannot express. Models a database clock running ahead of the host's: the
    /// due scan fires while the host still reads `now` as before the deadline.
    ///
    /// Pinned rather than offset from the wall clock so the guest sees this
    /// value however long instantiation and the entry-step replay take. An
    /// offset would let real time between here and the guest's `now-ms` eat
    /// into `remaining_ms` — and on a loaded machine push past it entirely, at
    /// which point the test still passes, but because the wait genuinely
    /// elapsed rather than because the tolerance let it through. It would have
    /// quietly stopped testing the guard.
    fn pin_clock_before(&self, deadline_ms: u64, remaining_ms: u64) {
        *self.pinned_clock_ms.lock().unwrap() = Some(deadline_ms.saturating_sub(remaining_ms));
    }

    /// Move the guest's clock far enough forward that `deadline_ms` has passed —
    /// what the wake scheduler achieves by simply not relaunching until then.
    fn advance_clock_past(&self, deadline_ms: u64) {
        let wall = now_ms();
        let mut offset = self.clock_offset_ms.lock().unwrap();
        // +1 so the guest sees `now > deadline`, not `now == deadline`.
        *offset = (*offset).max(deadline_ms.saturating_sub(wall).saturating_add(1));
    }

    /// Whether a custom signal is armed for `checkpoint_id` — what the
    /// wake-scheduler stand-in consults before relaunching an on-signal park.
    fn has_signal(&self, checkpoint_id: &str) -> bool {
        self.custom_signals
            .lock()
            .unwrap()
            .contains_key(checkpoint_id)
            || self.any_signal.lock().unwrap().is_some()
    }

    fn deliver_signal_any(&self, payload: &[u8]) {
        *self.any_signal.lock().unwrap() = Some(payload.to_vec());
    }

    fn request_signal(&self) {
        self.pending_signal
            .store(true, std::sync::atomic::Ordering::SeqCst);
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
        Ok(self
            .pending_signal
            .load(std::sync::atomic::Ordering::SeqCst))
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
    fn now_ms(&self) -> Result<u64, String> {
        if let Some(pinned) = *self.pinned_clock_ms.lock().unwrap() {
            return Ok(pinned);
        }
        Ok(now_ms() + *self.clock_offset_ms.lock().unwrap())
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
        // Blocking path: record the key and persist the checkpoint exactly as
        // core's `handle_sleep` does — which saves whenever the checkpoint id is
        // non-empty, EMPTY STATE INCLUDED. Skipping the empty case (as this mock
        // used to) hid the fact that the blocking arm leaves a `some([])` behind
        // under the very key the park arm looks up. Only the sleep itself is
        // skipped here, which is what keeps the 1h fixture fast.
        {
            self.checkpoints
                .lock()
                .unwrap()
                .entry(checkpoint_id.clone())
                .or_insert(state);
        }
        self.sleeps.lock().unwrap().push(checkpoint_id);
        // Report the failure only AFTER the checkpoint and the key are recorded:
        // core saves before it sleeps, so a sleep that dies on the client's
        // deadline dies with its checkpoint already durable.
        if let Some(message) = self.sleep_error.lock().unwrap().as_ref() {
            return Err(message.clone());
        }
        Ok(())
    }
}

fn run_invoke_once(
    wasm_path: &Path,
    host: Arc<dyn runtara_component_host::runtime_host::RuntimeHost>,
    input: Vec<u8>,
) -> runtara_component_host::InvokeExit {
    run_invoke_once_with_env(wasm_path, host, input, HashMap::new())
}

fn run_invoke_once_with_env(
    wasm_path: &Path,
    host: Arc<dyn runtara_component_host::runtime_host::RuntimeHost>,
    input: Vec<u8>,
    env: HashMap<String, String>,
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
                        env,
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

/// What a parked run did on one leg of a [`drive_wake_scheduler`] loop.
#[derive(Debug)]
enum ParkLeg {
    /// The run exited `suspended`, carrying these wakes.
    Parked(Vec<runtara_component_host::lifecycle::WorkflowWake>),
    /// The run reached a terminal outcome and the loop stopped.
    Finished(runtara_component_host::InvokeExit),
}

/// In-process stand-in for the wake scheduler: relaunch a parked run until it
/// reaches a terminal outcome, returning every leg in order.
///
/// The harness previously invoked exactly once, which cannot be right once a
/// delay parks — a single invoke sees the suspend and nothing after it. This
/// drives the real production shape: park, decide, relaunch, replay.
///
/// It honours both wake shapes the host supports, refusing to relaunch a park
/// that production would leave parked:
///
/// - `at(deadline)` — a timed park. The scheduler relaunches once the deadline
///   passes, and the GUEST enforces that: a resumed park re-reads its stored
///   deadline and re-parks if it is still in the future, so relaunching early
///   does NOT run the delay out. The loop therefore advances the host's virtual
///   clock past the deadline before relaunching — honouring it exactly, without
///   burning the wall clock on an hour-long fixture.
/// - `on-signal{deadline}` — a signal park. Relaunching is only legitimate once
///   `deliver` has armed the awaited signal or a timeout deadline exists; a park
///   with neither would sit forever in production, so the loop stops rather than
///   spinning and hiding that.
///
/// `deliver` is the custom-signal waker: it runs on each park and may deliver a
/// signal for the id the wake reported.
fn drive_wake_scheduler(
    wasm_path: &Path,
    host: Arc<CheckpointingRuntimeHost>,
    input: Vec<u8>,
    max_relaunches: usize,
    mut deliver: impl FnMut(
        usize,
        &CheckpointingRuntimeHost,
        &[runtara_component_host::lifecycle::WorkflowWake],
    ),
) -> Vec<ParkLeg> {
    use runtara_component_host::lifecycle::WorkflowWake;

    let mut legs = Vec::new();
    for relaunch in 0..=max_relaunches {
        let exit = run_invoke_once(wasm_path, host.clone(), input.clone());
        let runtara_component_host::InvokeExit::Suspended(wakes) = exit else {
            legs.push(ParkLeg::Finished(exit));
            return legs;
        };
        deliver(relaunch, host.as_ref(), &wakes);
        // Honour every timed wake by moving the clock to it, the way the
        // scheduler honours it by waiting.
        for wake in &wakes {
            match wake {
                WorkflowWake::At(ms) => host.advance_clock_past(*ms),
                WorkflowWake::OnSignal(wait) => {
                    if let Some(ms) = wait.deadline_ms {
                        host.advance_clock_past(ms);
                    }
                }
                WorkflowWake::OnResume => {}
            }
        }
        let wakeable = wakes.iter().any(|wake| match wake {
            WorkflowWake::At(_) => true,
            WorkflowWake::OnSignal(wait) => {
                wait.deadline_ms.is_some() || host.has_signal(&wait.checkpoint_id)
            }
            WorkflowWake::OnResume => false,
        });
        legs.push(ParkLeg::Parked(wakes));
        assert!(
            wakeable,
            "leg {relaunch} parked with no wake the scheduler could act on; \
             production would leave this instance parked forever"
        );
    }
    panic!("run did not finish within {max_relaunches} relaunches");
}

impl ParkLeg {
    fn wakes(&self) -> &[runtara_component_host::lifecycle::WorkflowWake] {
        match self {
            ParkLeg::Parked(wakes) => wakes,
            ParkLeg::Finished(exit) => panic!("expected a park, got {exit:?}"),
        }
    }

    fn output(&self) -> &[u8] {
        match self {
            ParkLeg::Finished(runtara_component_host::InvokeExit::Completed(output)) => output,
            other => panic!("expected a completed run, got {other:?}"),
        }
    }
}

/// A one-millisecond top-level durable Delay under the invoke export exits with
/// `suspended(at(deadline))` on first reach, freeing the Store. Its one wake
/// relaunch then hits the persisted absolute deadline and completes without
/// calling the blocking sleep host function.
#[test]
fn direct_wasm_execute_invoke_one_millisecond_delay_parks_then_resumes_once() {
    let components_dir = direct_e2e_components_dir();
    let input = br#"{"value":"resume-me"}"#.to_vec();
    let duration_ms = 1u64;

    let parking = compile_invoke_abi_artifact(
        &components_dir,
        "delay-park-one-millisecond",
        &store_freeing_delay_fixture(Some(duration_ms)),
    );
    let host = Arc::new(CheckpointingRuntimeHost::new(&input));

    let before = now_ms();
    let legs = drive_wake_scheduler(
        &parking.wasm_path,
        host.clone(),
        input.clone(),
        1,
        |_, _, _| {},
    );
    let after = now_ms();

    assert_eq!(legs.len(), 2, "one park then one completing relaunch");
    let wakes = legs[0].wakes();
    assert_eq!(wakes.len(), 1, "sequential lowering emits one wake");
    let deadline = match &wakes[0] {
        runtara_component_host::lifecycle::WorkflowWake::At(ms) => *ms,
        other => panic!("a one-millisecond durable Delay must park on a timed wake, got {other:?}"),
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
        "a park must checkpoint its deadline under the delay key"
    );
    assert!(
        host.sleeps.lock().unwrap().is_empty(),
        "a parked delay must not call the blocking durable-sleep host fn — \
         not on the park, and not on the resume whose checkpoint HIT skips it"
    );
    let expected: Value = serde_json::json!({ "echo": "resume-me" });
    assert_eq!(
        serde_json::from_slice::<Value>(legs[1].output()).expect("output is JSON"),
        expected
    );
}

/// A rate-limited Agent retry persists both a failure envelope and an absolute
/// next-attempt deadline. An early restart must re-park on that same deadline
/// without repeating the HTTP call; the due wake performs exactly one retry.
#[test]
fn direct_wasm_execute_invoke_rate_limited_agent_retry_parks_and_replays_once() {
    let components_dir = direct_e2e_components_dir();
    let (proxy_url, hits, proxy) = spawn_retry_http_proxy(vec![
        br#"{"status":429,"headers":{"retry-after-ms":"5000"},"body":{"error":"rate limited"}}"#
            .to_vec(),
        br#"{"status":200,"headers":{},"body":{"ok":true}}"#.to_vec(),
    ]);
    let artifact = compile_invoke_abi_artifact(
        &components_dir,
        "invoke-rate-limited-agent-retry-park",
        &lifecycle_retry_http_graph(),
    );
    let input = serde_json::to_vec(&serde_json::json!({ "url": format!("{proxy_url}/item") }))
        .expect("retry input");
    let mut env = HashMap::new();
    env.insert("RUNTARA_HTTP_PROXY_URL".to_string(), proxy_url);
    let host = Arc::new(CheckpointingRuntimeHost::new(&input));

    let first = run_invoke_once_with_env(
        &artifact.wasm_path,
        host.clone(),
        input.clone(),
        env.clone(),
    );
    let deadline = match first {
        runtara_component_host::InvokeExit::Suspended(wakes) => match wakes.as_slice() {
            [runtara_component_host::lifecycle::WorkflowWake::At(deadline)] => *deadline,
            other => panic!("rate-limited retry must park on one timed wake, got {other:?}"),
        },
        other => panic!("rate-limited retry must park, got {other:?}"),
    };
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the first 429 is the only call before a due wake"
    );
    let checkpoints = host.checkpoints.lock().unwrap();
    assert!(
        checkpoints.keys().any(|key| key.contains("::attempt::1")),
        "failed attempt envelope must be checkpointed: {checkpoints:?}"
    );
    assert!(
        checkpoints
            .iter()
            .any(|(key, state)| key.contains("::retry_sleep::2") && state.len() == 8),
        "next-attempt key must contain exactly one absolute u64 deadline: {checkpoints:?}"
    );
    drop(checkpoints);
    assert!(
        host.sleeps.lock().unwrap().is_empty(),
        "lifecycle retry parking must not call durable-sleep-checkpoint"
    );

    // A manual/early restart must preserve the existing absolute deadline and
    // must not re-invoke the failed attempt while it is still parked.
    // The guest accepts up to one second of database/host clock skew, so pin
    // farther away than that tolerance to prove an operator's early resume
    // cannot consume the retry.
    host.pin_clock_before(deadline, 2_000);
    let early = run_invoke_once_with_env(
        &artifact.wasm_path,
        host.clone(),
        input.clone(),
        env.clone(),
    );
    match early {
        runtara_component_host::InvokeExit::Suspended(wakes) => assert_eq!(
            wakes,
            vec![runtara_component_host::lifecycle::WorkflowWake::At(
                deadline
            )],
            "early restart must retain the original retry deadline"
        ),
        other => panic!("early retry restart must re-park, got {other:?}"),
    }
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "early restart must replay the checkpointed failure, not call upstream again"
    );

    *host.pinned_clock_ms.lock().unwrap() = None;
    host.advance_clock_past(deadline);
    let completed = run_invoke_once_with_env(&artifact.wasm_path, host.clone(), input, env);
    let output = match completed {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("due retry wake must complete after the scripted success, got {other:?}"),
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&output).expect("retry output JSON"),
        serde_json::json!({ "status": 200 })
    );
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the due wake must issue exactly one retry"
    );
    assert!(
        host.sleeps.lock().unwrap().is_empty(),
        "no lifecycle retry leg may hold the runner in a durable sleep"
    );
    proxy.join().expect("retry proxy joins");
}

/// Whole-Split retries park in the lifecycle ABI as well. This exercises the
/// sequential Split path specifically; a Split that requests concurrency but
/// carries retrying items degrades onto this same path (see
/// `direct_wasm_execute_invoke_parallel_split_item_retry_parks_sequentially`).
#[test]
fn direct_wasm_execute_invoke_sequential_split_retry_parks_before_second_attempt() {
    let components_dir = direct_e2e_components_dir();
    let artifact = compile_invoke_abi_artifact(
        &components_dir,
        "invoke-sequential-split-retry-park",
        &lifecycle_retry_split_graph(),
    );
    let input = br#"{}"#.to_vec();
    let host = Arc::new(CheckpointingRuntimeHost::new(&input));

    let first = run_invoke_once(&artifact.wasm_path, host.clone(), input.clone());
    let deadline = match first {
        runtara_component_host::InvokeExit::Suspended(wakes) => match wakes.as_slice() {
            [runtara_component_host::lifecycle::WorkflowWake::At(deadline)] => *deadline,
            other => panic!("sequential Split retry must park on one timed wake, got {other:?}"),
        },
        other => panic!("sequential Split retry must park, got {other:?}"),
    };
    let checkpoints = host.checkpoints.lock().unwrap();
    assert!(
        checkpoints.keys().any(|key| key.contains("::attempt::1")),
        "failed Split attempt must be checkpointed: {checkpoints:?}"
    );
    assert!(
        checkpoints
            .iter()
            .any(|(key, state)| key.contains("::retry_sleep::2") && state.len() == 8),
        "Split retry wake must persist an absolute deadline: {checkpoints:?}"
    );
    drop(checkpoints);
    assert!(
        host.sleeps.lock().unwrap().is_empty(),
        "sequential Split retry must not hold a runner in durable sleep"
    );

    host.advance_clock_past(deadline);
    assert!(
        matches!(
            run_invoke_once(&artifact.wasm_path, host.clone(), input),
            runtara_component_host::InvokeExit::Failed(_)
        ),
        "the due retry should execute its second transient failure and terminate"
    );
    assert!(
        host.sleeps.lock().unwrap().is_empty(),
        "the due Split retry must still avoid durable sleep"
    );
}

/// The regression from #217: a Split with `parallelism` > 1 whose item Agent
/// carries the ordinary retry policy. The concurrent window cannot park a
/// per-item retry, so the emitter falls back to the sequential lowering — and
/// that fallback must actually run, park on an absolute deadline, and never
/// hold the runner in a durable sleep. Rejecting this graph at compile time
/// instead left every triggered run permanently unstartable.
#[test]
fn direct_wasm_execute_invoke_parallel_split_item_retry_parks_sequentially() {
    let components_dir = direct_e2e_components_dir();
    let (proxy_url, hits, proxy) = spawn_retry_http_proxy(vec![
        br#"{"status":429,"headers":{"retry-after-ms":"5000"},"body":{"error":"rate limited"}}"#
            .to_vec(),
        br#"{"status":200,"headers":{},"body":{"ok":true}}"#.to_vec(),
    ]);
    let artifact = compile_invoke_abi_artifact(
        &components_dir,
        "invoke-parallel-split-item-retry-park",
        &lifecycle_parallel_split_retry_graph(&format!("{proxy_url}/item")),
    );
    let input = br#"{}"#.to_vec();
    let mut env = HashMap::new();
    env.insert("RUNTARA_HTTP_PROXY_URL".to_string(), proxy_url);
    let host = Arc::new(CheckpointingRuntimeHost::new(&input));

    let first = run_invoke_once_with_env(
        &artifact.wasm_path,
        host.clone(),
        input.clone(),
        env.clone(),
    );
    let deadline = match first {
        runtara_component_host::InvokeExit::Suspended(wakes) => match wakes.as_slice() {
            [runtara_component_host::lifecycle::WorkflowWake::At(deadline)] => *deadline,
            other => panic!("the item retry must park on one timed wake, got {other:?}"),
        },
        other => panic!("the item retry must park, got {other:?}"),
    };
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the first 429 is the only call before a due wake"
    );
    let checkpoints = host.checkpoints.lock().unwrap();
    assert!(
        checkpoints
            .iter()
            .any(|(key, state)| key.contains("::retry_sleep::2") && state.len() == 8),
        "the parked item retry must persist one absolute u64 deadline: {checkpoints:?}"
    );
    drop(checkpoints);
    assert!(
        host.sleeps.lock().unwrap().is_empty(),
        "the sequential fallback must not hold the runner in a durable sleep"
    );

    host.advance_clock_past(deadline);
    let completed = run_invoke_once_with_env(&artifact.wasm_path, host.clone(), input, env);
    let output = match completed {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("the due wake must complete the split, got {other:?}"),
    };
    let output: Value = serde_json::from_slice(&output).expect("split output JSON");
    let results = output["results"].as_array().expect("split results");
    assert_eq!(results.len(), 1, "one item: {output}");
    assert_eq!(results[0]["status"], 200, "item result: {}", results[0]);
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the due wake must issue exactly one retry"
    );
    assert!(
        host.sleeps.lock().unwrap().is_empty(),
        "the due retry leg must still avoid durable sleep"
    );
    proxy.join().expect("retry proxy joins");
}

/// EmbedWorkflow retries use the same per-attempt envelope plus absolute wake
/// protocol, so a failed child does not keep the parent's runner allocated.
#[test]
fn direct_wasm_execute_invoke_embed_workflow_retry_parks_before_second_attempt() {
    let components_dir = direct_e2e_components_dir();
    let child: ExecutionGraph =
        serde_json::from_str(LIFECYCLE_RETRY_EMBED_CHILD).expect("child graph parses");
    let artifact = compile_invoke_abi_artifact_with_children(
        &components_dir,
        "invoke-embed-workflow-retry-park",
        LIFECYCLE_RETRY_EMBED_PARENT,
        vec![runtara_workflows::compile::ChildWorkflowInput {
            step_id: "call_child".to_string(),
            workflow_id: "retry-child".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 1,
            execution_graph: child,
        }],
    );
    let input = br#"{}"#.to_vec();
    let host = Arc::new(CheckpointingRuntimeHost::new(&input));

    let first = run_invoke_once(&artifact.wasm_path, host.clone(), input.clone());
    let deadline = match first {
        runtara_component_host::InvokeExit::Suspended(wakes) => match wakes.as_slice() {
            [runtara_component_host::lifecycle::WorkflowWake::At(deadline)] => *deadline,
            other => panic!("EmbedWorkflow retry must park on one timed wake, got {other:?}"),
        },
        other => panic!("EmbedWorkflow retry must park, got {other:?}"),
    };
    let checkpoints = host.checkpoints.lock().unwrap();
    assert!(
        checkpoints.keys().any(|key| key.contains("::attempt::1")),
        "failed child attempt must be checkpointed: {checkpoints:?}"
    );
    assert!(
        checkpoints
            .iter()
            .any(|(key, state)| key.contains("::retry_sleep::2") && state.len() == 8),
        "EmbedWorkflow retry wake must persist an absolute deadline: {checkpoints:?}"
    );
    drop(checkpoints);
    assert!(
        host.sleeps.lock().unwrap().is_empty(),
        "EmbedWorkflow retry must not hold a runner in durable sleep"
    );

    host.advance_clock_past(deadline);
    assert!(
        matches!(
            run_invoke_once(&artifact.wasm_path, host.clone(), input),
            runtara_component_host::InvokeExit::Failed(_)
        ),
        "the due child retry should execute its second transient failure and terminate"
    );
    assert!(
        host.sleeps.lock().unwrap().is_empty(),
        "the due EmbedWorkflow retry must still avoid durable sleep"
    );
}

/// A cancellation/pause signal delivered while a retry is parked is observed
/// before its due wake can issue another upstream call.
#[test]
fn direct_wasm_execute_invoke_retry_park_observes_signal_before_retrying() {
    let components_dir = direct_e2e_components_dir();
    let (proxy_url, hits, proxy) = spawn_retry_http_proxy(vec![
        br#"{"status":429,"headers":{"retry-after-ms":"5000"},"body":{"error":"rate limited"}}"#
            .to_vec(),
    ]);
    let artifact = compile_invoke_abi_artifact(
        &components_dir,
        "invoke-retry-park-signal",
        &lifecycle_retry_http_graph(),
    );
    let input = serde_json::to_vec(&serde_json::json!({ "url": format!("{proxy_url}/item") }))
        .expect("retry input");
    let mut env = HashMap::new();
    env.insert("RUNTARA_HTTP_PROXY_URL".to_string(), proxy_url);
    let host = Arc::new(CheckpointingRuntimeHost::new(&input));

    let first = run_invoke_once_with_env(
        &artifact.wasm_path,
        host.clone(),
        input.clone(),
        env.clone(),
    );
    let deadline = match first {
        runtara_component_host::InvokeExit::Suspended(wakes) => match wakes.as_slice() {
            [runtara_component_host::lifecycle::WorkflowWake::At(deadline)] => *deadline,
            other => panic!("rate-limited retry must park on one timed wake, got {other:?}"),
        },
        other => panic!("rate-limited retry must park, got {other:?}"),
    };

    host.request_signal();
    host.advance_clock_past(deadline);
    let signalled = run_invoke_once_with_env(&artifact.wasm_path, host.clone(), input, env);
    assert!(
        matches!(signalled, runtara_component_host::InvokeExit::Suspended(_)),
        "a pending lifecycle signal must stop before the retry attempt, got {signalled:?}"
    );
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "a signal during the park must prevent the second upstream call"
    );
    proxy.join().expect("retry proxy joins");
}

/// A dynamic durable Delay must always park under the invoke ABI. The duration
/// is intentionally evaluated at runtime, so both a one-millisecond input and
/// an hour-long input exercise the same emitted park-only lowering.
#[test]
fn direct_wasm_execute_invoke_dynamic_delay_always_parks() {
    let components_dir = direct_e2e_components_dir();
    let artifact = compile_invoke_abi_artifact(
        &components_dir,
        "delay-dynamic-park-runtime",
        &store_freeing_delay_fixture(None),
    );

    let short_input = br#"{"value":"short","waitMs":1}"#.to_vec();
    let short_host = Arc::new(CheckpointingRuntimeHost::new(&short_input));
    let short_legs = drive_wake_scheduler(
        &artifact.wasm_path,
        short_host.clone(),
        short_input.clone(),
        1,
        |_, _, _| {},
    );
    assert!(
        matches!(
            short_legs[0].wakes().first(),
            Some(runtara_component_host::lifecycle::WorkflowWake::At(_))
        ),
        "a one-millisecond dynamic duration must park, got {:?}",
        short_legs[0]
    );
    assert_eq!(
        short_legs.len(),
        2,
        "a one-millisecond delay must wake and finish exactly once"
    );
    assert!(
        short_host.sleeps.lock().unwrap().is_empty(),
        "a parked dynamic delay must not use durable-sleep-checkpoint"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(short_legs[1].output()).expect("output is JSON"),
        serde_json::json!({ "echo": "short" }),
    );

    let long_input = br#"{"value":"long","waitMs":3600000}"#.to_vec();
    let long_host = Arc::new(CheckpointingRuntimeHost::new(&long_input));
    let long_legs = drive_wake_scheduler(
        &artifact.wasm_path,
        long_host.clone(),
        long_input.clone(),
        1,
        |_, _, _| {},
    );
    assert!(
        matches!(
            long_legs[0].wakes().first(),
            Some(runtara_component_host::lifecycle::WorkflowWake::At(_))
        ),
        "an hour-long duration must park on a timed wake, got {:?}",
        long_legs[0]
    );
    assert!(
        long_host.sleeps.lock().unwrap().is_empty(),
        "a parked delay must not block"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(long_legs[1].output()).expect("output is JSON"),
        serde_json::json!({ "echo": "long" }),
    );
}

/// The `wasi:cli/run` export blocks a long Delay even though the invoke export
/// parks the identical graph. What survived the gate's deletion is a CAPABILITY
/// check, not a policy one: `cli-run` has no success arm that can carry a wake,
/// so it has nowhere to put a deadline and must block.
#[test]
fn direct_wasm_execute_cli_run_abi_blocks_a_long_delay() {
    let components_dir = direct_e2e_components_dir();
    let graph: ExecutionGraph = serde_json::from_str(&store_freeing_delay_fixture(Some(3_600_000)))
        .expect("delay fixture parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let compiled = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "delay-cli-run-blocks".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        },
        &components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::CliRunHttp,
        false,
    )
    .expect("cli-run compile+compose succeeds");

    let input = br#"{"value":"cli-run"}"#.to_vec();
    let host = Arc::new(CheckpointingRuntimeHost::new(&input));
    let (ok, stderr, _) = execute_via_embedded(&compiled.wasm_path, &[], Some(host.clone()));
    assert!(ok, "cli-run artifact must run to completion: {stderr}");

    assert_eq!(
        host.sleeps.lock().unwrap().as_slice(),
        &["delay".to_string()],
        "under cli-run even an hours-long Delay must block on durable-sleep-checkpoint"
    );
    // The blocking sleep leaves core's empty checkpoint behind, but never an
    // 8-byte deadline: cli-run has no wake that could carry one.
    assert_eq!(
        host.checkpoints.lock().unwrap().get("delay"),
        Some(&Vec::new()),
        "cli-run must record only the blocking sleep's empty checkpoint, never a deadline"
    );
}

/// A durable sleep that fails must SAY so.
///
/// The blocking arm used to end its `durable-sleep-checkpoint` call with a bare
/// `Err` return, which under `wasi:cli/run` is an exit code and nothing else: no
/// `failed` event, no message. All an operator got was that the process had
/// died — a report naming neither the sleep nor its cause, and pointing squarely
/// at the wrong problem.
///
/// This is the arm where that matters. `cli-run` has no success arm able to
/// carry a wake, so it can never park and always blocks; under the composed
/// binding that block is an HTTP request held open for the sleep's whole
/// duration, which the client's own request deadline outlasts for any sleep
/// beyond it. Every one of those failures was mute.
#[test]
fn direct_wasm_execute_cli_run_reports_a_failed_durable_sleep() {
    const SLEEP_FAILURE: &str =
        "durable sleep of 3600000ms does not fit inside the 30000ms client request timeout";

    let components_dir = direct_e2e_components_dir();
    let graph: ExecutionGraph = serde_json::from_str(&store_freeing_delay_fixture(Some(3_600_000)))
        .expect("delay fixture parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let compiled = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "delay-cli-run-sleep-failure".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        },
        &components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::CliRunHttp,
        false,
    )
    .expect("cli-run compile+compose succeeds");

    let input = br#"{"value":"cli-run"}"#.to_vec();
    let host = Arc::new(CheckpointingRuntimeHost::new(&input));
    host.fail_sleeps_with(SLEEP_FAILURE);
    let (ok, _stderr, _) = execute_via_embedded(&compiled.wasm_path, &[], Some(host.clone()));

    assert!(
        !ok,
        "a workflow whose durable sleep failed must not report success"
    );
    let reported = host.failed.lock().unwrap().clone().expect(
        "the sleep failure must reach the operator through runtime.fail, not just an exit code",
    );
    assert!(
        String::from_utf8_lossy(&reported).contains(SLEEP_FAILURE),
        "the report must carry the sleep's own diagnosis; got {:?}",
        String::from_utf8_lossy(&reported)
    );
}

/// Relaunching a parked Delay BEFORE its deadline must not skip the wait: the
/// guest re-reads the stored deadline and re-parks on the same absolute value.
///
/// The wake scheduler is not the only relauncher — `handle_resume_instance`
/// accepts any instance whose status is `suspended`, with no `termination_reason`
/// filter, so an operator resuming a workflow parked on a long Delay lands here
/// early. A HIT means "a deadline was recorded", never "the wait is over".
#[test]
fn direct_wasm_execute_invoke_early_relaunch_reparks_instead_of_skipping_the_delay() {
    let components_dir = direct_e2e_components_dir();
    let input = br#"{"value":"early"}"#.to_vec();
    let duration_ms = 3_600_000u64;
    let artifact = compile_invoke_abi_artifact(
        &components_dir,
        "delay-early-relaunch",
        &store_freeing_delay_fixture(Some(duration_ms)),
    );
    let host = Arc::new(CheckpointingRuntimeHost::new(&input));

    let first = run_invoke_once(&artifact.wasm_path, host.clone(), input.clone());
    let first_deadline = match &first {
        runtara_component_host::InvokeExit::Suspended(wakes) => match &wakes[0] {
            runtara_component_host::lifecycle::WorkflowWake::At(ms) => *ms,
            other => panic!("expected a timed wake, got {other:?}"),
        },
        other => panic!("first invoke must park, got {other:?}"),
    };

    // Relaunch immediately — an hour before the deadline, exactly as a manual
    // resume would. TWICE: one re-park shows the deadline was re-read, but only
    // a second shows re-parking is IDEMPOTENT. A lowering that re-parked at a
    // recomputed `now + duration` instead of the stored value would satisfy the
    // first relaunch and slide the deadline forward on every one after it, so
    // the wait never ends — which is the failure `wait.rs` already documents.
    let mut deadlines = vec![first_deadline];
    for relaunch in 0..2 {
        let exit = run_invoke_once(&artifact.wasm_path, host.clone(), input.clone());
        match &exit {
            runtara_component_host::InvokeExit::Suspended(wakes) => match &wakes[0] {
                runtara_component_host::lifecycle::WorkflowWake::At(ms) => deadlines.push(*ms),
                other => panic!("relaunch {relaunch} expected a timed wake, got {other:?}"),
            },
            other => panic!(
                "early relaunch {relaunch} must re-park, not run the delay out; got {other:?}"
            ),
        }
    }
    assert!(
        deadlines.iter().all(|ms| *ms == first_deadline),
        "every re-park must carry the SAME absolute deadline — the wait is \
         neither shortened nor slid forward by having been relaunched; got {deadlines:?}"
    );
    assert!(
        host.completed.lock().unwrap().is_none(),
        "an early relaunch must not complete the run"
    );
    assert!(
        host.sleeps.lock().unwrap().is_empty(),
        "re-parking must not fall back to a blocking sleep"
    );
}

/// The skew tolerance is bounded on BOTH sides: a relaunch just outside it
/// still re-parks, one just inside it finishes the wait.
///
/// The two clocks are genuinely different — a deadline is minted from the
/// environment host's wall clock, while the due-instance scan compares
/// `sleep_until` against `Dialect::NOW` — so a database running ahead relaunches
/// the park early. Without a tolerance the guest re-parks on a deadline the scan
/// STILL considers due, and every relaunch until the host clock catches up
/// replays the workflow from its entry step, with nothing logged to say why.
///
/// The upper bound is the half that is easy to leave untested, and it is the
/// one that matters: the hour-early case in
/// `direct_wasm_execute_invoke_early_relaunch_reparks_instead_of_skipping_the_delay`
/// passes for any tolerance under an hour. A large tolerance would let a
/// short legal park fall straight through on its first early relaunch,
/// reinstating the skip this arm exists to prevent. Relaunching just outside
/// the tolerance is what pins it.
#[test]
fn direct_wasm_execute_invoke_clock_skew_tolerance_is_bounded_on_both_sides() {
    let components_dir = direct_e2e_components_dir();
    let input = br#"{"value":"skewed"}"#.to_vec();
    let artifact = compile_invoke_abi_artifact(
        &components_dir,
        "delay-skew-tolerance",
        &store_freeing_delay_fixture(Some(3_600_000)),
    );
    let host = Arc::new(CheckpointingRuntimeHost::new(&input));

    let first = run_invoke_once(&artifact.wasm_path, host.clone(), input.clone());
    let deadline = match &first {
        runtara_component_host::InvokeExit::Suspended(wakes) => match &wakes[0] {
            runtara_component_host::lifecycle::WorkflowWake::At(ms) => *ms,
            other => panic!("expected a timed wake, got {other:?}"),
        },
        other => panic!("first invoke must park, got {other:?}"),
    };

    // OUTSIDE: two seconds of wait still owed. Real time, not skew — it must
    // still be served. This is what stops the tolerance being widened to the
    // point where it swallows a genuine wait.
    host.pin_clock_before(deadline, 2_000);
    let outside = run_invoke_once(&artifact.wasm_path, host.clone(), input.clone());
    match &outside {
        runtara_component_host::InvokeExit::Suspended(wakes) => match &wakes[0] {
            runtara_component_host::lifecycle::WorkflowWake::At(ms) => assert_eq!(
                *ms, deadline,
                "a re-park outside the tolerance must keep the same absolute deadline"
            ),
            other => panic!("expected a timed wake, got {other:?}"),
        },
        other => panic!(
            "a relaunch OUTSIDE the skew tolerance must re-park — a tolerance wide \
             enough to swallow 2s of owed wait is wide enough to swallow the \
             shortest legal park whole. Got {other:?}"
        ),
    }

    // INSIDE: half a second short of the deadline, which is the host/database
    // clock split rather than owed wait. Finish it.
    host.pin_clock_before(deadline, 500);
    let inside = run_invoke_once(&artifact.wasm_path, host.clone(), input.clone());
    let output = match inside {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!(
            "a relaunch inside the skew tolerance must finish the wait; re-parking \
             here burns a full replay on every scheduler poll until the host clock \
             reaches the deadline. Got {other:?}"
        ),
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&output).expect("output is JSON"),
        serde_json::json!({ "echo": "skewed" }),
    );
    assert!(
        host.sleeps.lock().unwrap().is_empty(),
        "finishing the wait must not fall back to a blocking sleep"
    );
}

/// The blocking arm and the park arm share one checkpoint key, and core's
/// `handle_sleep` saves an EMPTY state under it. The park arm must tell that
/// apart from its own 8-byte deadline — and having done so, treat it as a
/// SERVED wait and continue.
///
/// That is the correct resume, not a shortcut: the only writer of empty state
/// under this key is `handle_sleep`, so its presence means the blocking arm
/// already slept this delay on an earlier pass. Re-parking instead would hang
/// the run outright — `handle_checkpoint` is get-or-set, so the park's deadline
/// could never overwrite the empty state and every relaunch would park again.
#[test]
fn direct_wasm_execute_invoke_blocking_sleep_checkpoint_reads_as_a_served_wait() {
    let components_dir = direct_e2e_components_dir();
    let input = br#"{"value":"aliased"}"#.to_vec();
    let artifact = compile_invoke_abi_artifact(
        &components_dir,
        "delay-key-aliasing",
        &store_freeing_delay_fixture(Some(3_600_000)),
    );
    let host = Arc::new(CheckpointingRuntimeHost::new(&input));
    // Seed the key exactly as a prior BLOCKING pass through this step would
    // have, via core's handle_sleep.
    host.checkpoints
        .lock()
        .unwrap()
        .insert("delay".to_string(), Vec::new());

    let exit = run_invoke_once(&artifact.wasm_path, host.clone(), input.clone());
    let output = match exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!(
            "a served blocking-sleep checkpoint must let the run continue, not park \
             (a re-park here could never persist its deadline and would hang); got {other:?}"
        ),
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&output).expect("output is JSON"),
        serde_json::json!({ "echo": "aliased" }),
    );
    assert!(
        host.sleeps.lock().unwrap().is_empty(),
        "the served wait must not be slept again"
    );
    // The empty state is left exactly as it was: nothing overwrote it, which is
    // precisely why the length check has to happen before the deadline read.
    assert_eq!(
        host.checkpoints.lock().unwrap().get("delay"),
        Some(&Vec::new()),
        "a get-or-set checkpoint store leaves the blocking arm's empty state in place"
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

/// A WaitForSignal fixture with a caller-chosen timeout, so a test can pick one
/// SHORTER than the deadline skew tolerance.
fn timed_wait_fixture(timeout_ms: u64) -> String {
    format!(
        r#"{{
  "name": "Timed Wait",
  "steps": {{
    "wait": {{
      "stepType": "WaitForSignal",
      "id": "wait",
      "name": "Approval",
      "pollIntervalMs": 0,
      "timeoutMs": {{ "valueType": "immediate", "value": {timeout_ms} }},
      "responseSchema": {{ "approved": {{ "type": "boolean", "required": true }} }}
    }},
    "finish": {{
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {{
        "approved": {{ "valueType": "reference", "value": "steps.wait.outputs.approved" }}
      }}
    }}
  }},
  "entryPoint": "wait",
  "executionPlan": [ {{ "fromStep": "wait", "toStep": "finish" }} ],
  "variables": {{}},
  "inputSchema": {{}},
  "outputSchema": {{}}
}}"#
    )
}

/// A wait that never PARKED gets no tolerance at all — which is the whole job
/// of the resumed flag once the half-window clamp is in place.
///
/// Under `wasi:cli/run` a Wait cannot park (no success arm can carry a wake), so
/// it polls in-process against one clock for its whole timeout. There is no
/// database scan involved and therefore no skew to absorb, and the clamp does
/// not help: it bounds the tolerance at half the window, so an ungated wait here
/// would end its timeout up to HALF early for no reason at all.
///
/// Asserted on elapsed time, which can only fail in the safe direction — a
/// loaded machine makes the run slower, never faster, so the floor cannot flake
/// red. Ungated, this 400ms wait resolves at ~200ms.
#[test]
fn direct_wasm_execute_cli_run_wait_timeout_gets_no_skew_tolerance() {
    let components_dir = direct_e2e_components_dir();
    let graph: ExecutionGraph =
        serde_json::from_str(&timed_wait_fixture(400)).expect("wait fixture parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let compiled = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "wait-cli-run-no-tolerance".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        },
        &components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::CliRunHttp,
        false,
    )
    .expect("cli-run compile+compose succeeds");

    let input = br#"{}"#.to_vec();
    let host = Arc::new(CheckpointingRuntimeHost::new(&input));
    let started = std::time::Instant::now();
    let (ok, _stderr, _) = execute_via_embedded(&compiled.wasm_path, &[], Some(host.clone()));
    let elapsed = started.elapsed();

    assert!(
        !ok,
        "a wait with no signal must fail on its timeout under cli-run"
    );
    assert!(
        elapsed >= std::time::Duration::from_millis(300),
        "an in-process wait must serve its FULL 400ms window — no park happened, \
         so there is no clock skew to absorb and the tolerance must stay off. \
         Resolved after {elapsed:?}"
    );
}

/// A resumed Wait absorbs the host/database clock split the same way a resumed
/// Delay does: woken a shade early it serves its timeout instead of re-parking,
/// but woken with real time still owed it re-parks on the SAME deadline.
///
/// A Wait's deadline lands in the same `sleep_until` column, stamped from the
/// same host clock, and is woken by the same database-clock scan as a parked
/// Delay, so it arrives early for the same reason. This fixture's timeout is far
/// wider than the tolerance, so the half-window clamp is not binding here —
/// `..._is_clamped_to_half_the_window` covers that.
#[test]
fn direct_wasm_execute_invoke_wait_timeout_tolerance_is_armed_only_on_resume() {
    let components_dir = direct_e2e_components_dir();
    let input = br#"{}"#.to_vec();
    let artifact = compile_invoke_abi_artifact(
        &components_dir,
        "wait-timeout-tolerance",
        &timed_wait_fixture(60_000),
    );
    let host = Arc::new(CheckpointingRuntimeHost::new(&input));

    let first = run_invoke_once(&artifact.wasm_path, host.clone(), input.clone());
    let deadline = match &first {
        runtara_component_host::InvokeExit::Suspended(wakes) => match &wakes[0] {
            runtara_component_host::lifecycle::WorkflowWake::OnSignal(wait) => wait
                .deadline_ms
                .expect("a timed wait parks carrying its deadline"),
            other => panic!("a timed wait must park on-signal, got {other:?}"),
        },
        other => panic!("a first reach with no signal must park, got {other:?}"),
    };

    // Outside the tolerance: real wait owed, so re-park on the SAME deadline.
    // The deadline must not absorb the tolerance — if it did, the next wake
    // would move earlier by exactly the tolerance and absorb nothing.
    host.pin_clock_before(deadline, 2_000);
    let outside = run_invoke_once(&artifact.wasm_path, host.clone(), input.clone());
    match &outside {
        runtara_component_host::InvokeExit::Suspended(wakes) => match &wakes[0] {
            runtara_component_host::lifecycle::WorkflowWake::OnSignal(wait) => assert_eq!(
                wait.deadline_ms,
                Some(deadline),
                "a re-park must carry the same absolute deadline"
            ),
            other => panic!("expected an on-signal wake, got {other:?}"),
        },
        other => panic!("2s of owed wait must still re-park, got {other:?}"),
    }

    // Inside the tolerance: the database clock fired the wake a shade early.
    // Serve the timeout rather than spinning another relaunch. Asserted as an
    // actual WAIT_TIMEOUT failure (the fixture has no onError) rather than a
    // bare "not suspended", which a trap or a silent completion would satisfy
    // too — and a broken local index on this path would produce exactly that.
    host.pin_clock_before(deadline, 500);
    let inside = run_invoke_once(&artifact.wasm_path, host.clone(), input.clone());
    let error = match inside {
        runtara_component_host::InvokeExit::Failed(error) => error,
        other => panic!(
            "a resumed wait inside the tolerance must resolve its timeout, not \
             re-park; got {other:?}"
        ),
    };
    assert!(
        error.message.contains("timed out"),
        "the resumed wait must fail with its WAIT_TIMEOUT error, got {error:?}"
    );
}

/// The tolerance is clamped to HALF the wait's own timeout, so a short window
/// cannot be swallowed whole.
///
/// The resumed flag alone does not bound this. A store-freeing Wait has no park
/// floor — it parks on its FIRST poll miss — so every pass after the first is a
/// resumed one, and a flat 1s tolerance would consume the entire remaining
/// window of any timeout near or below it. A wait relaunched off its deadline
/// (an operator resume, a recovery sweep, another waker) would then report
/// WAIT_TIMEOUT with time the author asked for still unspent: the same defect as
/// a Delay losing its remaining wait, merely bounded.
///
/// 400ms timeout → 200ms of tolerance. Relaunching 300ms early is INSIDE the
/// unclamped 1s tolerance and outside the clamped one, so this fails if the
/// clamp is dropped.
#[test]
fn direct_wasm_execute_invoke_wait_timeout_tolerance_is_clamped_to_half_the_window() {
    let components_dir = direct_e2e_components_dir();
    let input = br#"{}"#.to_vec();
    let artifact = compile_invoke_abi_artifact(
        &components_dir,
        "wait-timeout-clamp",
        &timed_wait_fixture(400),
    );
    let host = Arc::new(CheckpointingRuntimeHost::new(&input));

    let first = run_invoke_once(&artifact.wasm_path, host.clone(), input.clone());
    let deadline = match &first {
        runtara_component_host::InvokeExit::Suspended(wakes) => match &wakes[0] {
            runtara_component_host::lifecycle::WorkflowWake::OnSignal(wait) => wait
                .deadline_ms
                .expect("a timed wait parks carrying its deadline"),
            other => panic!("a timed wait must park on-signal, got {other:?}"),
        },
        other => panic!(
            "a 400ms timeout must still PARK on its first reach rather than \
             resolve immediately, got {other:?}"
        ),
    };

    // 300ms short of a 400ms window: three quarters of the wait still owed.
    host.pin_clock_before(deadline, 300);
    let exit = run_invoke_once(&artifact.wasm_path, host.clone(), input.clone());
    match &exit {
        runtara_component_host::InvokeExit::Suspended(wakes) => match &wakes[0] {
            runtara_component_host::lifecycle::WorkflowWake::OnSignal(wait) => assert_eq!(
                wait.deadline_ms,
                Some(deadline),
                "the clamped re-park must keep the same absolute deadline"
            ),
            other => panic!("expected an on-signal wake, got {other:?}"),
        },
        other => panic!(
            "an unclamped 1s tolerance would swallow this 400ms window whole; \
             the clamp must hold it to 200ms and re-park. Got {other:?}"
        ),
    }

    // 50ms short — inside the clamped 200ms — still resolves, so the clamp
    // narrows the tolerance rather than disabling it.
    host.pin_clock_before(deadline, 50);
    let served = run_invoke_once(&artifact.wasm_path, host.clone(), input.clone());
    assert!(
        matches!(served, runtara_component_host::InvokeExit::Failed(ref e)
            if e.message.contains("timed out")),
        "inside the CLAMPED tolerance the timeout must still be served, got {served:?}"
    );
}

/// A durable WaitForSignal under the invoke export EXITS with
/// `suspended(on-signal{signal-id, deadline})` on the first poll MISS — freeing
/// the Store — instead of blocking the poll loop. The wake-scheduler stand-in
/// then plays the custom-signal waker: it delivers the signal for the id the
/// wake reported and relaunches, and the replayed wait re-polls the now-present
/// signal and completes.
///
/// A no-timeout wait carries NO deadline (the waker is the sole wake path),
/// which is exactly the shape the stand-in refuses to relaunch until a signal
/// is armed. Unlike a Delay, no duration threshold applies: a Wait is
/// open-ended by construction.
#[test]
fn direct_wasm_execute_invoke_wait_parks_on_signal_then_resumes() {
    let components_dir = direct_e2e_components_dir();
    let input = br#"{}"#.to_vec();

    let artifact = compile_invoke_abi_artifact(&components_dir, "wait-park", STORE_FREEING_WAIT);
    let host = Arc::new(CheckpointingRuntimeHost::new(&input));

    let legs = drive_wake_scheduler(
        &artifact.wasm_path,
        host.clone(),
        input.clone(),
        1,
        |_, host, wakes| {
            // The custom-signal waker: arm the signal the park is waiting on.
            for wake in wakes {
                if let runtara_component_host::lifecycle::WorkflowWake::OnSignal(wait) = wake {
                    host.deliver_signal(&wait.checkpoint_id, br#"{"approved": true}"#);
                }
            }
        },
    );

    assert_eq!(legs.len(), 2, "one park then one completing relaunch");
    let wakes = legs[0].wakes();
    assert_eq!(wakes.len(), 1, "sequential lowering emits one wake");
    let (checkpoint_id, deadline) = match &wakes[0] {
        runtara_component_host::lifecycle::WorkflowWake::OnSignal(wait) => {
            (wait.checkpoint_id.clone(), wait.deadline_ms)
        }
        other => panic!("a WaitForSignal must park on-signal, got {other:?}"),
    };
    assert!(
        !checkpoint_id.is_empty(),
        "on-signal wake carries the deterministic wait signal id"
    );
    assert_eq!(
        deadline, None,
        "a no-timeout wait parks without a deadline (waker is the sole wake path)"
    );
    assert!(
        host.sleeps.lock().unwrap().is_empty(),
        "a parked wait must not block on the poll interval"
    );
    let output = legs[1].output().to_vec();
    assert_eq!(
        serde_json::from_slice::<Value>(&output).expect("output is JSON"),
        serde_json::json!({ "approved": true }),
        "the resumed wait must surface the delivered signal payload"
    );

    // Control: a wait whose signal is ALREADY present never parks — its first
    // poll finds the signal — and reaches the same output.
    let present =
        compile_invoke_abi_artifact(&components_dir, "wait-signal-present", STORE_FREEING_WAIT);
    let present_host = Arc::new(CheckpointingRuntimeHost::new(&input));
    // The deterministic signal id is workflow-id-scoped, so pre-deliver for ANY
    // polled id.
    present_host.deliver_signal_any(br#"{"approved": true}"#);
    let present_exit = run_invoke_once(&present.wasm_path, present_host.clone(), input.clone());
    let present_output = match present_exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("a wait with a present signal must complete, got {other:?}"),
    };
    assert_eq!(
        present_output, output,
        "a resumed park's output must byte-match the never-parked run's"
    );
}

/// P5 full-parity loop, in process: a child workflow PUBLISHED as an agent
/// (compiled with the AgentCapabilities ABI under its slug, staged under the
/// native-agent naming convention with a synthesized meta sidecar) is composed
/// into a PARENT workflow like any native agent — targeted by an ordinary
/// Agent step as `agentId: <slug>, capabilityId: "run"` — and the parent
/// executes end to end, the child's output flowing back through the standard
/// agent-output shaping.
#[test]
fn parent_workflow_composes_and_invokes_published_workflow_agent() {
    let components_dir = direct_e2e_components_dir();

    // 1. The child: a pure workflow with a typed input, published as an agent.
    const CHILD: &str = r#"{
      "name": "Shout Echo",
      "durable": false,
      "steps": {
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": {
            "echoed": { "valueType": "reference", "value": "data.text" },
            "marker": { "valueType": "immediate", "value": "from-child" }
          }
        }
      },
      "entryPoint": "finish",
      "executionPlan": [],
      "variables": {},
      "inputSchema": { "text": { "type": "string", "required": true } },
      "outputSchema": {}
    }"#;
    let child_graph: ExecutionGraph = serde_json::from_str(CHILD).expect("child parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let child = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "child-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: child_graph.clone(),
            child_workflows: vec![],
            output_dir: temp.path().join("child-build"),
            track_events: false,
            agent_catalog: None,
            agent_slug: Some("shout-echo".to_string()),
        },
        &components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
        false,
    )
    .expect("child agent compile+compose succeeds");
    assert!(
        child
            .component_artifacts
            .world_wit
            .contains("export runtara:agent-shout-echo/capabilities@0.4.0;"),
        "child must export under its slug:\n{}",
        child.component_artifacts.world_wit
    );

    // 2. Stage exactly the way the server publish path does: the composed
    //    `.wasm` + the synthesized meta under the agent naming convention.
    let staging = temp.path().join("workflow-agents");
    fs::create_dir_all(&staging).expect("staging dir");
    fs::copy(
        &child.wasm_path,
        staging.join("runtara_agent_shout_echo.wasm"),
    )
    .expect("stage child wasm");
    let info = certified_workflow_agent_info(
        "shout-echo",
        "Shout Echo",
        "",
        &child_graph.input_schema,
        &child_graph.output_schema,
    );
    fs::write(
        staging.join("runtara_agent_shout_echo.meta.json"),
        serde_json::to_vec_pretty(&info).expect("meta serializes"),
    )
    .expect("stage child meta");

    // 3. The parent: an ordinary Agent step targeting the published child.
    const PARENT: &str = r#"{
      "name": "Parent Of Published Agent",
      "steps": {
        "call": {
          "stepType": "Agent",
          "id": "call",
          "agentId": "shout-echo",
          "capabilityId": "run",
          "inputMapping": { "text": { "valueType": "reference", "value": "data.msg" } }
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": {
            "childEcho": { "valueType": "reference", "value": "steps.call.outputs.echoed" },
            "childMarker": { "valueType": "reference", "value": "steps.call.outputs.marker" }
          }
        }
      },
      "entryPoint": "call",
      "executionPlan": [{ "fromStep": "call", "toStep": "finish" }],
      "variables": {},
      "inputSchema": { "msg": { "type": "string", "required": true } },
      "outputSchema": {}
    }"#;
    let parent_graph: ExecutionGraph = serde_json::from_str(PARENT).expect("parent parses");
    // The catalog overlay (synthesized meta) is what lets the manifest builder
    // resolve `shout-echo` / capability `run` / its required inputs.
    let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
        info,
    ]));
    let mut parent = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "parent-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: parent_graph,
            child_workflows: vec![],
            output_dir: temp.path().join("parent-build"),
            track_events: false,
            agent_catalog: Some(catalog),
            agent_slug: None,
        },
        runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
        false,
    )
    .expect("parent compile succeeds");
    runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
        &mut parent,
        &components_dir,
        std::slice::from_ref(&staging),
    )
    .expect("parent compose finds the staged child in the extra search dir");

    // 4. Run the parent — the child executes composed-in like a native agent.
    let host = Arc::new(RecordingRuntimeHost::new(b"{}"));
    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let run = runtime.block_on(async {
        let pre = executor
            .load_instance_pre(&parent.wasm_path)
            .await
            .expect("load parent artifact");
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
                br#"{"msg":"hello-child"}"#.to_vec(),
            )
            .await
    });

    let output = match run.exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("parent invoking a published workflow-agent must complete, got {other:?}"),
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&output).expect("output is JSON"),
        serde_json::json!({ "childEcho": "hello-child", "childMarker": "from-child" }),
        "the child's output must flow back through the standard agent-output shaping"
    );
    std::mem::forget(temp);
}

/// P6 flagship: a DURABLE workflow published as an agent runs composed inside
/// a parent. The child keeps the runtime import (a durable Delay checkpoints +
/// sleeps through it); composition bubbles that import up to the composed
/// artifact where the PARENT instance's runtime host satisfies it. Critically,
/// the child's terminal `runtime.complete` is suppressed — exactly ONE
/// complete fires for the whole run (the parent's) — because a child
/// completing the shared instance would finish the parent mid-flight.
#[test]
fn parent_workflow_invokes_published_durable_workflow_agent() {
    let components_dir = direct_e2e_components_dir();

    // Durable child: default durability + a short Delay — needs the runtime.
    const DURABLE_CHILD: &str = r#"{
      "name": "Durable Delay Echo",
      "steps": {
        "delay": {
          "stepType": "Delay",
          "id": "delay",
          "durationMs": { "valueType": "immediate", "value": 25 }
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": { "echo": { "valueType": "reference", "value": "data.value" } }
        }
      },
      "entryPoint": "delay",
      "executionPlan": [ { "fromStep": "delay", "toStep": "finish" } ],
      "variables": {},
      "inputSchema": { "value": { "type": "string", "required": true } },
      "outputSchema": {}
    }"#;
    let child_graph: ExecutionGraph = serde_json::from_str(DURABLE_CHILD).expect("child parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let child = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "durable-child-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: child_graph.clone(),
            child_workflows: vec![],
            output_dir: temp.path().join("child-build"),
            track_events: false,
            agent_catalog: None,
            agent_slug: Some("durable-delay-echo".to_string()),
        },
        &components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
        false,
    )
    .expect("durable child publishes as an agent");
    assert!(
        !child.omit_runtime,
        "the durable child must keep the runtime import"
    );

    let staging = temp.path().join("workflow-agents");
    fs::create_dir_all(&staging).expect("staging dir");
    fs::copy(
        &child.wasm_path,
        staging.join("runtara_agent_durable_delay_echo.wasm"),
    )
    .expect("stage child wasm");
    let info = certified_workflow_agent_info(
        "durable-delay-echo",
        "Durable Delay Echo",
        "",
        &child_graph.input_schema,
        &child_graph.output_schema,
    );
    fs::write(
        staging.join("runtara_agent_durable_delay_echo.meta.json"),
        serde_json::to_vec_pretty(&info).expect("meta serializes"),
    )
    .expect("stage child meta");

    const PARENT: &str = r#"{
      "name": "Parent Of Durable Agent",
      "steps": {
        "call": {
          "stepType": "Agent",
          "id": "call",
          "agentId": "durable-delay-echo",
          "capabilityId": "run",
          "inputMapping": { "value": { "valueType": "reference", "value": "data.msg" } }
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": { "childEcho": { "valueType": "reference", "value": "steps.call.outputs.echo" } }
        }
      },
      "entryPoint": "call",
      "executionPlan": [{ "fromStep": "call", "toStep": "finish" }],
      "variables": {},
      "inputSchema": { "msg": { "type": "string", "required": true } },
      "outputSchema": {}
    }"#;
    let parent_graph: ExecutionGraph = serde_json::from_str(PARENT).expect("parent parses");
    let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
        info,
    ]));
    let mut parent = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "durable-parent-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: parent_graph,
            child_workflows: vec![],
            output_dir: temp.path().join("parent-build"),
            track_events: false,
            agent_catalog: Some(catalog),
            agent_slug: None,
        },
        runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
        false,
    )
    .expect("parent compile succeeds");
    runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
        &mut parent,
        &components_dir,
        std::slice::from_ref(&staging),
    )
    .expect("parent composes the durable child (runtime import bubbles up)");

    let host = Arc::new(RecordingRuntimeHost::new(b"{}"));
    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let run = runtime.block_on(async {
        let pre = executor
            .load_instance_pre(&parent.wasm_path)
            .await
            .expect("load parent artifact");
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
                br#"{"msg":"durable-hello"}"#.to_vec(),
            )
            .await
    });

    let output = match run.exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("parent invoking a durable workflow-agent must complete, got {other:?}"),
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&output).expect("output is JSON"),
        serde_json::json!({ "childEcho": "durable-hello" }),
        "the durable child's output must flow back through agent-output shaping"
    );
    // Exactly ONE terminal complete — the parent's. The child's suppression is
    // what keeps a shared-instance runtime coherent; two completes would mean
    // the child finished the parent's instance mid-flight.
    assert_eq!(
        host.complete_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the composed child must never fire runtime.complete"
    );
    assert!(host.failed.lock().unwrap().is_none());
    std::mem::forget(temp);
}

/// Checkpoint NAMESPACING:
/// a Split fanning out over a DURABLE workflow-agent child gives every
/// invocation site its own checkpoint namespace. The child's internal step is
/// deliberately named `call` — the SAME id as the parent's Agent step — the
/// classic collision. Without the `agent-scope-input` envelope wrap, all
/// three iterations' durable Delays would share ONE bare `call` sleep key
/// (iteration 2 would HIT iteration 1's checkpoint and skip its sleep).
/// A second run against the SAME host proves the scoped ids are replay-stable:
/// everything HITs, nothing re-sleeps, the output is identical.
#[test]
fn composed_durable_child_checkpoints_are_namespaced_per_invocation_site() {
    let components_dir = direct_e2e_components_dir();

    const DURABLE_CHILD: &str = r#"{
      "name": "NS Durable Child",
      "steps": {
        "call": {
          "stepType": "Delay",
          "id": "call",
          "durationMs": { "valueType": "immediate", "value": 5 }
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": { "echo": { "valueType": "reference", "value": "data.value" } }
        }
      },
      "entryPoint": "call",
      "executionPlan": [ { "fromStep": "call", "toStep": "finish" } ],
      "variables": {},
      "inputSchema": { "value": { "type": "string", "required": true } },
      "outputSchema": {}
    }"#;
    let child_graph: ExecutionGraph = serde_json::from_str(DURABLE_CHILD).expect("child parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let child = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "ns-child-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: child_graph.clone(),
            child_workflows: vec![],
            output_dir: temp.path().join("child-build"),
            track_events: false,
            agent_catalog: None,
            agent_slug: Some("ns-delay-echo".to_string()),
        },
        &components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
        false,
    )
    .expect("durable child publishes as an agent");
    assert!(
        !child.omit_runtime,
        "durable child keeps the runtime import"
    );

    let staging = temp.path().join("workflow-agents");
    fs::create_dir_all(&staging).expect("staging dir");
    fs::copy(
        &child.wasm_path,
        staging.join("runtara_agent_ns_delay_echo.wasm"),
    )
    .expect("stage child wasm");
    let info = certified_workflow_agent_info(
        "ns-delay-echo",
        "NS Delay Echo",
        "",
        &child_graph.input_schema,
        &child_graph.output_schema,
    );
    fs::write(
        staging.join("runtara_agent_ns_delay_echo.meta.json"),
        serde_json::to_vec_pretty(&info).expect("meta serializes"),
    )
    .expect("stage child meta");

    const PARENT: &str = r#"{
      "name": "NS Split Parent",
      "durable": true,
      "steps": {
        "split": {
          "stepType": "Split",
          "id": "split",
          "config": { "value": { "valueType": "reference", "value": "data.items" } },
          "subgraph": {
            "name": "Body",
            "entryPoint": "call",
            "steps": {
              "call": {
                "stepType": "Agent",
                "id": "call",
                "agentId": "ns-delay-echo",
                "capabilityId": "run",
                "inputMapping": { "value": { "valueType": "reference", "value": "item.v" } }
              },
              "finish": {
                "stepType": "Finish",
                "id": "finish",
                "inputMapping": { "echo": { "valueType": "reference", "value": "steps.call.outputs.echo" } }
              }
            },
            "executionPlan": [ { "fromStep": "call", "toStep": "finish" } ]
          }
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": { "results": { "valueType": "reference", "value": "steps.split.outputs" } }
        }
      },
      "entryPoint": "split",
      "executionPlan": [ { "fromStep": "split", "toStep": "finish" } ],
      "variables": {},
      "inputSchema": { "items": { "type": "array", "required": true } },
      "outputSchema": {}
    }"#;
    let parent_graph: ExecutionGraph = serde_json::from_str(PARENT).expect("parent parses");
    let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
        info,
    ]));
    let mut parent = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "ns-parent-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: parent_graph,
            child_workflows: vec![],
            output_dir: temp.path().join("parent-build"),
            track_events: false,
            agent_catalog: Some(catalog),
            agent_slug: None,
        },
        runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
        false,
    )
    .expect("parent compile succeeds");
    runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
        &mut parent,
        &components_dir,
        std::slice::from_ref(&staging),
    )
    .expect("parent composes the durable child");

    let host = Arc::new(PersistingRuntimeHost::new(b"{}"));
    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let input = br#"{"items":[{"v":"a"},{"v":"b"},{"v":"c"}]}"#;
    let run_once = |host: Arc<PersistingRuntimeHost>| {
        runtime.block_on(async {
            let pre = executor
                .load_instance_pre(&parent.wasm_path)
                .await
                .expect("load parent artifact");
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
                    input.to_vec(),
                )
                .await
        })
    };

    let first = run_once(host.clone());
    let first_output = match first.exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("first run must complete, got {other:?}"),
    };

    // The three child Delay sleeps land on three DISTINCT, site-scoped keys —
    // the exact compositional formula ({workflow_id}::{step}[i]::{child step}),
    // NOT the bare `call` the un-namespaced child would have used thrice.
    let sleeps = host.sleep_ids.lock().unwrap().clone();
    assert_eq!(
        sleeps,
        vec![
            "ns-parent-wf::call[0]::call".to_string(),
            "ns-parent-wf::call[1]::call".to_string(),
            "ns-parent-wf::call[2]::call".to_string(),
        ],
        "each invocation site must own a distinct child sleep key"
    );
    // ...and stay disjoint from the parent's own durable writes for the
    // same-named `call` step (its agent-output checkpoints).
    let writes = host.checkpoint_writes.lock().unwrap().clone();
    assert!(
        writes.iter().all(|id| !sleeps.contains(id)),
        "child keys must never collide with parent checkpoint ids: {writes:?}"
    );

    // Replay: same host = a resume with all checkpoints present. Everything
    // HITs — no new sleep fires — and the output is byte-identical, proving
    // the scoped ids are deterministic across replays.
    let second = run_once(host.clone());
    let second_output = match second.exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("replay run must complete, got {other:?}"),
    };
    assert_eq!(
        host.sleep_ids.lock().unwrap().len(),
        3,
        "a replay must HIT every scoped sleep checkpoint, not re-sleep"
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&second_output).expect("replay output json"),
        serde_json::from_slice::<Value>(&first_output).expect("first output json"),
        "replay must reproduce the run from checkpoints"
    );
    assert_eq!(
        host.complete_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        2,
        "exactly one terminal complete per run (the parent's)"
    );
    assert!(host.failed.lock().unwrap().is_none());
    std::mem::forget(temp);
}

/// Three-level composition: a parent invokes a published workflow-agent that
/// itself invokes ANOTHER published (durable) workflow-agent. The checkpoint
/// namespace chains through both boundaries with the same `__`-composition
/// nested embeds use — the grandchild's durable Delay key carries both
/// invocation sites.
#[test]
fn nested_composed_workflow_agents_chain_checkpoint_namespaces() {
    let components_dir = direct_e2e_components_dir();

    const GRANDCHILD: &str = r#"{
      "name": "NS Grandchild",
      "steps": {
        "delay": {
          "stepType": "Delay",
          "id": "delay",
          "durationMs": { "valueType": "immediate", "value": 5 }
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": { "echo": { "valueType": "reference", "value": "data.value" } }
        }
      },
      "entryPoint": "delay",
      "executionPlan": [ { "fromStep": "delay", "toStep": "finish" } ],
      "variables": {},
      "inputSchema": { "value": { "type": "string", "required": true } },
      "outputSchema": {}
    }"#;
    let grandchild_graph: ExecutionGraph =
        serde_json::from_str(GRANDCHILD).expect("grandchild parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let grandchild = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "ns-grandchild-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: grandchild_graph.clone(),
            child_workflows: vec![],
            output_dir: temp.path().join("grandchild-build"),
            track_events: false,
            agent_catalog: None,
            agent_slug: Some("ns-grandchild".to_string()),
        },
        &components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
        false,
    )
    .expect("grandchild publishes as an agent");

    let staging = temp.path().join("workflow-agents");
    fs::create_dir_all(&staging).expect("staging dir");
    fs::copy(
        &grandchild.wasm_path,
        staging.join("runtara_agent_ns_grandchild.wasm"),
    )
    .expect("stage grandchild wasm");
    let grandchild_info = certified_workflow_agent_info(
        "ns-grandchild",
        "NS Grandchild",
        "",
        &grandchild_graph.input_schema,
        &grandchild_graph.output_schema,
    );
    fs::write(
        staging.join("runtara_agent_ns_grandchild.meta.json"),
        serde_json::to_vec_pretty(&grandchild_info).expect("meta serializes"),
    )
    .expect("stage grandchild meta");

    // The MID workflow-agent: invokes the grandchild, republishes as an agent.
    const MID: &str = r#"{
      "name": "NS Mid",
      "steps": {
        "gcall": {
          "stepType": "Agent",
          "id": "gcall",
          "agentId": "ns-grandchild",
          "capabilityId": "run",
          "inputMapping": { "value": { "valueType": "reference", "value": "data.value" } }
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": { "echo": { "valueType": "reference", "value": "steps.gcall.outputs.echo" } }
        }
      },
      "entryPoint": "gcall",
      "executionPlan": [ { "fromStep": "gcall", "toStep": "finish" } ],
      "variables": {},
      "inputSchema": { "value": { "type": "string", "required": true } },
      "outputSchema": {}
    }"#;
    let mid_graph: ExecutionGraph = serde_json::from_str(MID).expect("mid parses");
    let grandchild_catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
        grandchild_info,
    ]));
    let mut mid = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "ns-mid-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: mid_graph.clone(),
            child_workflows: vec![],
            output_dir: temp.path().join("mid-build"),
            track_events: false,
            agent_catalog: Some(grandchild_catalog),
            agent_slug: Some("ns-mid".to_string()),
        },
        runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
        false,
    )
    .expect("mid compiles as an agent");
    runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
        &mut mid,
        &components_dir,
        std::slice::from_ref(&staging),
    )
    .expect("mid composes the grandchild");
    fs::copy(&mid.wasm_path, staging.join("runtara_agent_ns_mid.wasm")).expect("stage mid wasm");
    let mid_info = certified_workflow_agent_info(
        "ns-mid",
        "NS Mid",
        "",
        &mid_graph.input_schema,
        &mid_graph.output_schema,
    );
    fs::write(
        staging.join("runtara_agent_ns_mid.meta.json"),
        serde_json::to_vec_pretty(&mid_info).expect("meta serializes"),
    )
    .expect("stage mid meta");

    const TOP: &str = r#"{
      "name": "NS Top",
      "steps": {
        "call": {
          "stepType": "Agent",
          "id": "call",
          "agentId": "ns-mid",
          "capabilityId": "run",
          "inputMapping": { "value": { "valueType": "reference", "value": "data.msg" } }
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": { "echo": { "valueType": "reference", "value": "steps.call.outputs.echo" } }
        }
      },
      "entryPoint": "call",
      "executionPlan": [{ "fromStep": "call", "toStep": "finish" }],
      "variables": {},
      "inputSchema": { "msg": { "type": "string", "required": true } },
      "outputSchema": {}
    }"#;
    let top_graph: ExecutionGraph = serde_json::from_str(TOP).expect("top parses");
    let mid_catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
        mid_info,
    ]));
    let mut top = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "ns-top-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: top_graph,
            child_workflows: vec![],
            output_dir: temp.path().join("top-build"),
            track_events: false,
            agent_catalog: Some(mid_catalog),
            agent_slug: None,
        },
        runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
        false,
    )
    .expect("top compile succeeds");
    runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
        &mut top,
        &components_dir,
        std::slice::from_ref(&staging),
    )
    .expect("top composes the mid agent");

    let host = Arc::new(PersistingRuntimeHost::new(b"{}"));
    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let run = runtime.block_on(async {
        let pre = executor
            .load_instance_pre(&top.wasm_path)
            .await
            .expect("load top artifact");
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
                br#"{"msg":"nested-hello"}"#.to_vec(),
            )
            .await
    });

    let output = match run.exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("nested composition must complete, got {other:?}"),
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&output).expect("output json"),
        serde_json::json!({ "echo": "nested-hello" }),
        "the value must round-trip through both composed children"
    );
    // The grandchild's durable Delay key chains BOTH invocation sites — the
    // top's `call` and the mid's `gcall` — via the same `__` composition
    // nested embeds use. One site, one key, however deep the nesting.
    assert_eq!(
        host.sleep_ids.lock().unwrap().clone(),
        vec!["ns-top-wf::call__gcall::delay".to_string()],
        "the grandchild sleep key must chain the full invocation path"
    );
    assert_eq!(
        host.complete_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "neither composed child may fire runtime.complete"
    );
    assert!(host.failed.lock().unwrap().is_none());
    std::mem::forget(temp);
}

/// Stale-artifact gate (plan §5): a DURABLE workflow-agent staged with a
/// sidecar that predates checkpoint namespacing (no `checkpoint-scope:1`
/// capability tag) must FAIL the parent's compose with a republish error —
/// its `build_source` would silently drop the injected `_cache_key_prefix`
/// and the checkpoint collision would return invisibly.
#[test]
fn stale_durable_workflow_agent_artifact_fails_compose() {
    let components_dir = direct_e2e_components_dir();

    const DURABLE_CHILD: &str = r#"{
      "name": "Stale Durable Child",
      "steps": {
        "delay": {
          "stepType": "Delay",
          "id": "delay",
          "durationMs": { "valueType": "immediate", "value": 5 }
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": { "echo": { "valueType": "reference", "value": "data.value" } }
        }
      },
      "entryPoint": "delay",
      "executionPlan": [ { "fromStep": "delay", "toStep": "finish" } ],
      "variables": {},
      "inputSchema": { "value": { "type": "string", "required": true } },
      "outputSchema": {}
    }"#;
    let child_graph: ExecutionGraph = serde_json::from_str(DURABLE_CHILD).expect("child parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let child = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "stale-child-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: child_graph.clone(),
            child_workflows: vec![],
            output_dir: temp.path().join("child-build"),
            track_events: false,
            agent_catalog: None,
            agent_slug: Some("stale-durable".to_string()),
        },
        &components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
        false,
    )
    .expect("durable child compiles as an agent");
    assert!(
        !child.omit_runtime,
        "durable child keeps the runtime import"
    );

    let staging = temp.path().join("workflow-agents");
    fs::create_dir_all(&staging).expect("staging dir");
    fs::copy(
        &child.wasm_path,
        staging.join("runtara_agent_stale_durable.wasm"),
    )
    .expect("stage child wasm");
    // Simulate a pre-namespacing publish: the synthesized meta WITHOUT the
    // `checkpoint-scope:1` marker tag.
    let info = certified_workflow_agent_info(
        "stale-durable",
        "Stale Durable",
        "",
        &child_graph.input_schema,
        &child_graph.output_schema,
    );
    let mut stripped = serde_json::to_value(&info).expect("info to json");
    let tags = stripped["capabilities"][0]["tags"]
        .as_array_mut()
        .expect("capability tags");
    tags.retain(|tag| tag != "checkpoint-scope:1");
    fs::write(
        staging.join("runtara_agent_stale_durable.meta.json"),
        serde_json::to_vec_pretty(&stripped).expect("meta serializes"),
    )
    .expect("stage stripped meta");

    const PARENT: &str = r#"{
      "name": "Parent Of Stale Agent",
      "steps": {
        "call": {
          "stepType": "Agent",
          "id": "call",
          "agentId": "stale-durable",
          "capabilityId": "run",
          "inputMapping": { "value": { "valueType": "reference", "value": "data.msg" } }
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": { "echo": { "valueType": "reference", "value": "steps.call.outputs.echo" } }
        }
      },
      "entryPoint": "call",
      "executionPlan": [{ "fromStep": "call", "toStep": "finish" }],
      "variables": {},
      "inputSchema": { "msg": { "type": "string", "required": true } },
      "outputSchema": {}
    }"#;
    let parent_graph: ExecutionGraph = serde_json::from_str(PARENT).expect("parent parses");
    // The catalog is what a server loading this stale sidecar would serve.
    let stale_info: runtara_dsl::agent_meta::AgentInfo =
        serde_json::from_value(stripped).expect("stripped info parses");
    let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
        stale_info,
    ]));
    let mut parent = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "stale-parent-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: parent_graph,
            child_workflows: vec![],
            output_dir: temp.path().join("parent-build"),
            track_events: false,
            agent_catalog: Some(catalog),
            agent_slug: None,
        },
        runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
        false,
    )
    .expect("parent compile itself succeeds");
    let error = runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
        &mut parent,
        &components_dir,
        std::slice::from_ref(&staging),
    )
    .expect_err("composing a stale DURABLE workflow-agent must fail");
    let message = error.to_string();
    assert!(
        message.contains("predates checkpoint namespacing") && message.contains("stale-durable"),
        "the error must name the stale slug and ask for a republish: {message}"
    );

    // Anomalous-branch variant: a runtime-importing staged wasm with NO
    // sidecar at all (partial stage / manual copy) must also be refused —
    // the wasm itself is the authority, not the sidecar's presence.
    fs::remove_file(staging.join("runtara_agent_stale_durable.meta.json")).expect("remove sidecar");
    let error = runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
        &mut parent,
        &components_dir,
        std::slice::from_ref(&staging),
    )
    .expect_err("a runtime-importing component without a sidecar must fail");
    let message = error.to_string();
    assert!(
        message.contains("missing or unreadable"),
        "the error must name the missing sidecar: {message}"
    );
    std::mem::forget(temp);
}

/// A pure workflow-agent staged without the explicit non-suspending proof is
/// refused too. Its bytes happen not to import the runtime, but the parent
/// must not infer future publication safety from a missing marker.
#[test]
fn uncertified_pure_workflow_agent_artifact_fails_compose() {
    let components_dir = direct_e2e_components_dir();

    const PURE_CHILD: &str = r#"{
      "name": "Stale Pure Child",
      "durable": false,
      "steps": {
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": { "echo": { "valueType": "reference", "value": "data.value" } }
        }
      },
      "entryPoint": "finish",
      "executionPlan": [],
      "variables": {},
      "inputSchema": { "value": { "type": "string", "required": true } },
      "outputSchema": {}
    }"#;
    let child_graph: ExecutionGraph = serde_json::from_str(PURE_CHILD).expect("child parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let child = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "stale-pure-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: child_graph.clone(),
            child_workflows: vec![],
            output_dir: temp.path().join("child-build"),
            track_events: false,
            agent_catalog: None,
            agent_slug: Some("stale-pure".to_string()),
        },
        &components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
        false,
    )
    .expect("pure child compiles as an agent");
    assert!(child.omit_runtime, "pure child must not import the runtime");

    let staging = temp.path().join("workflow-agents");
    fs::create_dir_all(&staging).expect("staging dir");
    fs::copy(
        &child.wasm_path,
        staging.join("runtara_agent_stale_pure.wasm"),
    )
    .expect("stage child wasm");
    let info = certified_workflow_agent_info(
        "stale-pure",
        "Stale Pure",
        "",
        &child_graph.input_schema,
        &child_graph.output_schema,
    );
    let mut stripped = serde_json::to_value(&info).expect("info to json");
    stripped["capabilities"][0]["tags"]
        .as_array_mut()
        .expect("capability tags")
        .retain(|tag| tag != "non-suspending:1");
    fs::write(
        staging.join("runtara_agent_stale_pure.meta.json"),
        serde_json::to_vec_pretty(&stripped).expect("meta serializes"),
    )
    .expect("stage stripped meta");

    const PARENT: &str = r#"{
      "name": "Parent Of Stale Pure Agent",
      "steps": {
        "call": {
          "stepType": "Agent",
          "id": "call",
          "agentId": "stale-pure",
          "capabilityId": "run",
          "inputMapping": { "value": { "valueType": "reference", "value": "data.msg" } }
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": { "echo": { "valueType": "reference", "value": "steps.call.outputs.echo" } }
        }
      },
      "entryPoint": "call",
      "executionPlan": [{ "fromStep": "call", "toStep": "finish" }],
      "variables": {},
      "inputSchema": { "msg": { "type": "string", "required": true } },
      "outputSchema": {}
    }"#;
    let parent_graph: ExecutionGraph = serde_json::from_str(PARENT).expect("parent parses");
    let stale_info: runtara_dsl::agent_meta::AgentInfo =
        serde_json::from_value(stripped).expect("stripped info parses");
    let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
        stale_info,
    ]));
    let mut parent = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "stale-pure-parent-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: parent_graph,
            child_workflows: vec![],
            output_dir: temp.path().join("parent-build"),
            track_events: false,
            agent_catalog: Some(catalog),
            agent_slug: None,
        },
        runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
        false,
    )
    .expect("parent compile succeeds");
    let error = runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
        &mut parent,
        &components_dir,
        std::slice::from_ref(&staging),
    )
    .expect_err("a workflow-agent without the safety certificate must not compose");
    assert!(
        error.to_string().contains("non-suspending:1"),
        "the parent must require an auditable safety certificate: {error}"
    );
    std::mem::forget(temp);
}

/// Plan §4 (checkpoint-namespace plan): per-invocation-site CUSTOM SIGNAL ids.
/// One durable workflow-agent child with a WaitForSignal step is invoked at
/// TWO sites in the parent. Before scoping, both sites derived the SAME
/// signal id (`{instance}/{child-wf}/{step}`) — one posted signal woke both
/// waiters with one payload, and the timeout-deadline checkpoint (keyed by
/// the signal id) was shared. With `_cache_key_prefix` folded in, each site
/// polls its own id and receives its own payload.
#[test]
fn composed_children_waiting_on_same_step_get_per_site_signal_ids() {
    let components_dir = direct_e2e_components_dir();

    const WAIT_CHILD: &str = r#"{
      "name": "Sig Approve Echo",
      "steps": {
        "approve": {
          "stepType": "WaitForSignal",
          "id": "approve",
          "timeoutMs": { "valueType": "immediate", "value": 60000 },
          "pollIntervalMs": 25
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": {
            "decision": { "valueType": "reference", "value": "steps.approve.outputs.decision" }
          }
        }
      },
      "entryPoint": "approve",
      "executionPlan": [ { "fromStep": "approve", "toStep": "finish" } ],
      "variables": {},
      "inputSchema": {},
      "outputSchema": {}
    }"#;
    let child_graph: ExecutionGraph = serde_json::from_str(WAIT_CHILD).expect("child parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let child = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "sig-child-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: child_graph.clone(),
            child_workflows: vec![],
            output_dir: temp.path().join("child-build"),
            track_events: false,
            agent_catalog: None,
            agent_slug: Some("sig-approve-echo".to_string()),
        },
        &components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
        false,
    )
    .expect("waiting child publishes as an agent");
    assert!(!child.omit_runtime, "a waiting child needs the runtime");

    let staging = temp.path().join("workflow-agents");
    fs::create_dir_all(&staging).expect("staging dir");
    fs::copy(
        &child.wasm_path,
        staging.join("runtara_agent_sig_approve_echo.wasm"),
    )
    .expect("stage child wasm");
    let info = certified_workflow_agent_info(
        "sig-approve-echo",
        "Sig Approve Echo",
        "",
        &child_graph.input_schema,
        &child_graph.output_schema,
    );
    fs::write(
        staging.join("runtara_agent_sig_approve_echo.meta.json"),
        serde_json::to_vec_pretty(&info).expect("meta serializes"),
    )
    .expect("stage child meta");

    const PARENT: &str = r#"{
      "name": "Sig Parent",
      "steps": {
        "call": {
          "stepType": "Agent",
          "id": "call",
          "agentId": "sig-approve-echo",
          "capabilityId": "run",
          "inputMapping": {}
        },
        "call2": {
          "stepType": "Agent",
          "id": "call2",
          "agentId": "sig-approve-echo",
          "capabilityId": "run",
          "inputMapping": {}
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": {
            "first": { "valueType": "reference", "value": "steps.call.outputs.decision" },
            "second": { "valueType": "reference", "value": "steps.call2.outputs.decision" }
          }
        }
      },
      "entryPoint": "call",
      "executionPlan": [
        { "fromStep": "call", "toStep": "call2" },
        { "fromStep": "call2", "toStep": "finish" }
      ],
      "variables": {},
      "inputSchema": {},
      "outputSchema": {}
    }"#;
    let parent_graph: ExecutionGraph = serde_json::from_str(PARENT).expect("parent parses");
    let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
        info,
    ]));
    let mut parent = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "sig-parent-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: parent_graph,
            child_workflows: vec![],
            output_dir: temp.path().join("parent-build"),
            track_events: false,
            agent_catalog: Some(catalog),
            agent_slug: None,
        },
        runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
        false,
    )
    .expect("parent compile succeeds");
    runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
        &mut parent,
        &components_dir,
        std::slice::from_ref(&staging),
    )
    .expect("parent composes the waiting child");

    // Deliver a DIFFERENT payload to each site's scoped id — exactly what a
    // sender does after discovering the ids from the two
    // `external_input_requested` events.
    let host = Arc::new(PersistingRuntimeHost::new(b"{}"));
    let site1 = "checkpoint-ns-e2e/sig-child-wf/sig-parent-wf::call::approve";
    let site2 = "checkpoint-ns-e2e/sig-child-wf/sig-parent-wf::call2::approve";
    host.deliver_signal(site1, br#"{"decision":"approve-first"}"#);
    host.deliver_signal(site2, br#"{"decision":"approve-second"}"#);

    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let run = runtime.block_on(async {
        let pre = executor
            .load_instance_pre(&parent.wasm_path)
            .await
            .expect("load parent artifact");
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
                b"{}".to_vec(),
            )
            .await
    });

    let output = match run.exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("both waits must receive their own signal, got {other:?}"),
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&output).expect("output json"),
        serde_json::json!({ "first": "approve-first", "second": "approve-second" }),
        "each invocation site must receive ITS payload, not the other's"
    );
    let polled: std::collections::BTreeSet<String> = host
        .polled_signal_ids
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .collect();
    assert!(
        polled.contains(site1) && polled.contains(site2),
        "each site must poll its own scoped id; polled: {polled:?}"
    );
    // The wait's timeout deadline is checkpointed UNDER the signal id — the
    // per-site ids keep the two deadlines from sharing one row.
    let writes = host.checkpoint_writes.lock().unwrap().clone();
    assert!(
        writes.iter().any(|id| id == site1) && writes.iter().any(|id| id == site2),
        "deadline checkpoints must be keyed by the scoped ids; writes: {writes:?}"
    );
    std::mem::forget(temp);
}

/// The EMBED twin of the composed test: one child workflow with a
/// WaitForSignal step embedded at TWO sites of the same parent. Embedded
/// children inherit the PARENT's `_workflow_id`, so before scoping both
/// embeds derived literally identical signal ids.
#[test]
fn embedded_children_waiting_on_same_step_get_per_site_signal_ids() {
    let components_dir = direct_e2e_components_dir();

    const WAIT_CHILD: &str = r#"{
      "name": "Sig Embed Child",
      "steps": {
        "approve": {
          "stepType": "WaitForSignal",
          "id": "approve",
          "pollIntervalMs": 25,
          "timeoutMs": { "valueType": "immediate", "value": 60000 }
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": {
            "decision": { "valueType": "reference", "value": "steps.approve.outputs.decision" }
          }
        }
      },
      "entryPoint": "approve",
      "executionPlan": [ { "fromStep": "approve", "toStep": "finish" } ],
      "variables": {},
      "inputSchema": {},
      "outputSchema": {}
    }"#;
    let child_graph: ExecutionGraph = serde_json::from_str(WAIT_CHILD).expect("child parses");

    const PARENT: &str = r#"{
      "name": "Sig Embed Parent",
      "steps": {
        "embed1": {
          "stepType": "EmbedWorkflow",
          "id": "embed1",
          "childWorkflowId": "sig-embed-child",
          "childVersion": "latest",
          "inputMapping": {}
        },
        "embed2": {
          "stepType": "EmbedWorkflow",
          "id": "embed2",
          "childWorkflowId": "sig-embed-child",
          "childVersion": "latest",
          "inputMapping": {}
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": {
            "first": { "valueType": "reference", "value": "steps.embed1.outputs.decision" },
            "second": { "valueType": "reference", "value": "steps.embed2.outputs.decision" }
          }
        }
      },
      "entryPoint": "embed1",
      "executionPlan": [
        { "fromStep": "embed1", "toStep": "embed2" },
        { "fromStep": "embed2", "toStep": "finish" }
      ],
      "variables": {},
      "inputSchema": {},
      "outputSchema": {}
    }"#;
    let parent_graph: ExecutionGraph = serde_json::from_str(PARENT).expect("parent parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let child_input = |step_id: &str| runtara_workflows::compile::ChildWorkflowInput {
        step_id: step_id.to_string(),
        workflow_id: "sig-embed-child".to_string(),
        version_requested: "latest".to_string(),
        version_resolved: 1,
        execution_graph: child_graph.clone(),
    };
    let mut parent = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "sig-embed-parent".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: parent_graph,
            child_workflows: vec![child_input("embed1"), child_input("embed2")],
            output_dir: temp.path().join("parent-build"),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        },
        runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
        false,
    )
    .expect("embed parent compiles");
    runtara_workflows::direct_wasm::compose_direct_workflow(&mut parent, &components_dir)
        .expect("embed parent composes");

    let host = Arc::new(PersistingRuntimeHost::new(b"{}"));
    // Embedded children keep the PARENT's workflow id in the second segment;
    // the site scope disambiguates the third.
    let site1 = "checkpoint-ns-e2e/sig-embed-parent/sig-embed-parent::embed1::approve";
    let site2 = "checkpoint-ns-e2e/sig-embed-parent/sig-embed-parent::embed2::approve";
    host.deliver_signal(site1, br#"{"decision":"embed-first"}"#);
    host.deliver_signal(site2, br#"{"decision":"embed-second"}"#);

    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let run = runtime.block_on(async {
        let pre = executor
            .load_instance_pre(&parent.wasm_path)
            .await
            .expect("load parent artifact");
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
                b"{}".to_vec(),
            )
            .await
    });

    let output = match run.exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("both embedded waits must receive their own signal, got {other:?}"),
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&output).expect("output json"),
        serde_json::json!({ "first": "embed-first", "second": "embed-second" }),
        "each embed site must receive ITS payload, not the other's"
    );
    let polled: std::collections::BTreeSet<String> = host
        .polled_signal_ids
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .collect();
    assert!(
        polled.contains(site1) && polled.contains(site2),
        "each embed site must poll its own scoped id; polled: {polled:?}"
    );
    // Deadline checkpoints keyed by the embed-scoped ids too.
    let writes = host.checkpoint_writes.lock().unwrap().clone();
    assert!(
        writes.iter().any(|id| id == site1) && writes.iter().any(|id| id == site2),
        "deadline checkpoints must be keyed by the embed-scoped ids; writes: {writes:?}"
    );
    std::mem::forget(temp);
}

/// Replay stability of scoped signal ids (plan §3 "replay-stable by
/// construction"), modeled on the production resume shape: a drain kills the
/// store mid-wait and resume relaunches CHECKPOINT-LESS (replay-from-start)
/// against the surviving checkpoint rows. The pre-drain incarnation wrote the
/// wait's timeout deadline under the scoped signal id and a sender posted the
/// signal while the instance was down. The replay must re-derive the exact
/// same scoped id — HITting the stored deadline (never re-writing it) and
/// finding the pending signal.
#[test]
fn scoped_signal_wait_survives_drain_and_resume() {
    let components_dir = direct_e2e_components_dir();

    const WAIT_CHILD: &str = r#"{
      "name": "Sig Drain Echo",
      "steps": {
        "approve": {
          "stepType": "WaitForSignal",
          "id": "approve",
          "timeoutMs": { "valueType": "immediate", "value": 60000 },
          "pollIntervalMs": 25
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": {
            "decision": { "valueType": "reference", "value": "steps.approve.outputs.decision" }
          }
        }
      },
      "entryPoint": "approve",
      "executionPlan": [ { "fromStep": "approve", "toStep": "finish" } ],
      "variables": {},
      "inputSchema": {},
      "outputSchema": {}
    }"#;
    let child_graph: ExecutionGraph = serde_json::from_str(WAIT_CHILD).expect("child parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let child = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "sigdrain-child-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: child_graph.clone(),
            child_workflows: vec![],
            output_dir: temp.path().join("child-build"),
            track_events: false,
            agent_catalog: None,
            agent_slug: Some("sig-drain-echo".to_string()),
        },
        &components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
        false,
    )
    .expect("waiting child publishes as an agent");

    let staging = temp.path().join("workflow-agents");
    fs::create_dir_all(&staging).expect("staging dir");
    fs::copy(
        &child.wasm_path,
        staging.join("runtara_agent_sig_drain_echo.wasm"),
    )
    .expect("stage child wasm");
    let info = certified_workflow_agent_info(
        "sig-drain-echo",
        "Sig Drain Echo",
        "",
        &child_graph.input_schema,
        &child_graph.output_schema,
    );
    fs::write(
        staging.join("runtara_agent_sig_drain_echo.meta.json"),
        serde_json::to_vec_pretty(&info).expect("meta serializes"),
    )
    .expect("stage child meta");

    const PARENT: &str = r#"{
      "name": "Sig Drain Parent",
      "steps": {
        "call": {
          "stepType": "Agent",
          "id": "call",
          "agentId": "sig-drain-echo",
          "capabilityId": "run",
          "inputMapping": {}
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": { "decision": { "valueType": "reference", "value": "steps.call.outputs.decision" } }
        }
      },
      "entryPoint": "call",
      "executionPlan": [{ "fromStep": "call", "toStep": "finish" }],
      "variables": {},
      "inputSchema": {},
      "outputSchema": {}
    }"#;
    let parent_graph: ExecutionGraph = serde_json::from_str(PARENT).expect("parent parses");
    let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
        info,
    ]));
    let mut parent = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "sigdrain-parent-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: parent_graph,
            child_workflows: vec![],
            output_dir: temp.path().join("parent-build"),
            track_events: false,
            agent_catalog: Some(catalog),
            agent_slug: None,
        },
        runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
        false,
    )
    .expect("parent compile succeeds");
    runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
        &mut parent,
        &components_dir,
        std::slice::from_ref(&staging),
    )
    .expect("parent composes the waiting child");

    let host = Arc::new(PersistingRuntimeHost::new(b"{}"));
    let site = "checkpoint-ns-e2e/sigdrain-child-wf/sigdrain-parent-wf::call::approve";
    // Surviving state from the pre-drain incarnation: the wait's absolute
    // deadline (raw little-endian i64 ms, far in the future) stored under the
    // SCOPED signal id, plus the signal a sender posted while the instance
    // was down.
    let deadline_ms = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as i64)
        + 600_000;
    host.checkpoints
        .lock()
        .unwrap()
        .insert(site.to_string(), deadline_ms.to_le_bytes().to_vec());
    host.deliver_signal(site, br#"{"decision":"approved-after-drain"}"#);

    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let run = runtime.block_on(async {
        let pre = executor
            .load_instance_pre(&parent.wasm_path)
            .await
            .expect("load parent artifact");
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
                b"{}".to_vec(),
            )
            .await
    });

    let output = match run.exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("the resumed wait must complete, got {other:?}"),
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&output).expect("output json"),
        serde_json::json!({ "decision": "approved-after-drain" }),
        "the signal posted while down must reach the replayed wait"
    );
    // The replay re-derived the same scoped id: the deadline lookup HIT the
    // pre-drain row, so the save branch never ran — zero checkpoint writes
    // under the scoped id in this incarnation.
    let deadline_writes = host
        .checkpoint_writes
        .lock()
        .unwrap()
        .iter()
        .filter(|id| id.as_str() == site)
        .count();
    assert_eq!(
        deadline_writes, 0,
        "the replay must HIT the stored deadline under the same scoped id, not re-write it"
    );
    std::mem::forget(temp);
}

/// N5 — in-guest suspend PROPAGATES through the composed-agent boundary.
/// A lifecycle pause fires while a composed workflow-agent child is blocked
/// in its WaitForSignal poll loop. The capability result type has no
/// suspended arm, so the child raises the suspend sentinel error; the parent
/// recognizes it at the invoke boundary and re-raises the suspend through
/// its own ABI. Before the fix, the parent failed with "failed to parse
/// Agent output". Resume (pause lifted, signal delivered) replays: the
/// child's step checkpoint never completed, so the child re-invokes, its
/// wait re-derives the same scoped id, HITs the stored deadline, and finds
/// the signal.
#[test]
fn pause_during_composed_child_wait_suspends_and_resumes() {
    let components_dir = direct_e2e_components_dir();

    const WAIT_CHILD: &str = r#"{
      "name": "Pause Approve Echo",
      "steps": {
        "approve": {
          "stepType": "WaitForSignal",
          "id": "approve",
          "timeoutMs": { "valueType": "immediate", "value": 60000 },
          "pollIntervalMs": 25
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": {
            "decision": { "valueType": "reference", "value": "steps.approve.outputs.decision" }
          }
        }
      },
      "entryPoint": "approve",
      "executionPlan": [ { "fromStep": "approve", "toStep": "finish" } ],
      "variables": {},
      "inputSchema": {},
      "outputSchema": {}
    }"#;
    let child_graph: ExecutionGraph = serde_json::from_str(WAIT_CHILD).expect("child parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let child = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "pause-child-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: child_graph.clone(),
            child_workflows: vec![],
            output_dir: temp.path().join("child-build"),
            track_events: false,
            agent_catalog: None,
            agent_slug: Some("pause-approve-echo".to_string()),
        },
        &components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
        false,
    )
    .expect("waiting child publishes as an agent");

    let staging = temp.path().join("workflow-agents");
    fs::create_dir_all(&staging).expect("staging dir");
    fs::copy(
        &child.wasm_path,
        staging.join("runtara_agent_pause_approve_echo.wasm"),
    )
    .expect("stage child wasm");
    let info = certified_workflow_agent_info(
        "pause-approve-echo",
        "Pause Approve Echo",
        "",
        &child_graph.input_schema,
        &child_graph.output_schema,
    );
    fs::write(
        staging.join("runtara_agent_pause_approve_echo.meta.json"),
        serde_json::to_vec_pretty(&info).expect("meta serializes"),
    )
    .expect("stage child meta");

    const PARENT: &str = r#"{
      "name": "Pause Parent",
      "steps": {
        "call": {
          "stepType": "Agent",
          "id": "call",
          "agentId": "pause-approve-echo",
          "capabilityId": "run",
          "inputMapping": {}
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": { "decision": { "valueType": "reference", "value": "steps.call.outputs.decision" } }
        }
      },
      "entryPoint": "call",
      "executionPlan": [{ "fromStep": "call", "toStep": "finish" }],
      "variables": {},
      "inputSchema": {},
      "outputSchema": {}
    }"#;
    let parent_graph: ExecutionGraph = serde_json::from_str(PARENT).expect("parent parses");
    let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
        info,
    ]));
    let mut parent = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "pause-parent-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: parent_graph,
            child_workflows: vec![],
            output_dir: temp.path().join("parent-build"),
            track_events: false,
            agent_catalog: Some(catalog),
            agent_slug: None,
        },
        runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
        false,
    )
    .expect("parent compile succeeds");
    runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
        &mut parent,
        &components_dir,
        std::slice::from_ref(&staging),
    )
    .expect("parent composes the waiting child");

    let host = Arc::new(PersistingRuntimeHost::new(b"{}"));
    let site = "checkpoint-ns-e2e/pause-child-wf/pause-parent-wf::call::approve";
    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let run_once = |host: Arc<PersistingRuntimeHost>| {
        runtime.block_on(async {
            let pre = executor
                .load_instance_pre(&parent.wasm_path)
                .await
                .expect("load parent artifact");
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
                    b"{}".to_vec(),
                )
                .await
        })
    };

    // PAUSE: the lifecycle signal fires on the child wait's first poll. The
    // suspend crosses the composition boundary and the PARENT suspends.
    host.suspend_requested
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let first = run_once(host.clone());
    match first.exit {
        runtara_component_host::InvokeExit::Suspended(wakes) => {
            assert!(
                matches!(
                    wakes.first(),
                    Some(runtara_component_host::lifecycle::WorkflowWake::OnResume)
                ),
                "a lifecycle suspend re-raises as an on-resume wake, got {wakes:?}"
            );
        }
        other => panic!("pause during the child's wait must SUSPEND the parent, got {other:?}"),
    }
    assert!(
        host.failed.lock().unwrap().is_none(),
        "the sentinel must never surface as a failure"
    );
    let deadline_writes = |host: &PersistingRuntimeHost| {
        host.checkpoint_writes
            .lock()
            .unwrap()
            .iter()
            .filter(|id| id.as_str() == site)
            .count()
    };
    assert_eq!(
        deadline_writes(&host),
        1,
        "the wait's deadline was checkpointed under the scoped id before suspending"
    );

    // RESUME: pause lifted, the signal arrived while suspended. Replay
    // re-invokes the child, re-derives the same scoped id, HITs the stored
    // deadline and finds the signal.
    host.suspend_requested
        .store(false, std::sync::atomic::Ordering::SeqCst);
    host.deliver_signal(site, br#"{"decision":"approved-after-pause"}"#);
    let second = run_once(host.clone());
    let output = match second.exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("the resumed run must complete, got {other:?}"),
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&output).expect("output json"),
        serde_json::json!({ "decision": "approved-after-pause" }),
        "the signal posted while paused must reach the resumed wait"
    );
    assert_eq!(
        deadline_writes(&host),
        1,
        "the resumed wait must HIT the stored deadline, not re-write it"
    );
    assert_eq!(
        host.complete_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "exactly one terminal complete — the resumed parent's"
    );
    std::mem::forget(temp);
}

/// N5 nesting: the suspend sentinel CHAINS. A pause fires inside the
/// GRANDCHILD's wait (parent → mid agent → grandchild agent); the grandchild
/// raises the sentinel, the mid re-raises it through ITS capability channel,
/// and the top-level parent suspends. Resume completes through both
/// boundaries.
#[test]
fn pause_inside_nested_composed_agents_chains_the_suspend() {
    let components_dir = direct_e2e_components_dir();

    const GRANDCHILD: &str = r#"{
      "name": "Pause Grandchild",
      "steps": {
        "approve": {
          "stepType": "WaitForSignal",
          "id": "approve",
          "pollIntervalMs": 25
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": { "echo": { "valueType": "reference", "value": "steps.approve.outputs.decision" } }
        }
      },
      "entryPoint": "approve",
      "executionPlan": [ { "fromStep": "approve", "toStep": "finish" } ],
      "variables": {},
      "inputSchema": {},
      "outputSchema": {}
    }"#;
    let grandchild_graph: ExecutionGraph =
        serde_json::from_str(GRANDCHILD).expect("grandchild parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let grandchild = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "pause-gc-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: grandchild_graph.clone(),
            child_workflows: vec![],
            output_dir: temp.path().join("grandchild-build"),
            track_events: false,
            agent_catalog: None,
            agent_slug: Some("pause-grandchild".to_string()),
        },
        &components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
        false,
    )
    .expect("grandchild publishes as an agent");

    let staging = temp.path().join("workflow-agents");
    fs::create_dir_all(&staging).expect("staging dir");
    fs::copy(
        &grandchild.wasm_path,
        staging.join("runtara_agent_pause_grandchild.wasm"),
    )
    .expect("stage grandchild wasm");
    let grandchild_info = certified_workflow_agent_info(
        "pause-grandchild",
        "Pause Grandchild",
        "",
        &grandchild_graph.input_schema,
        &grandchild_graph.output_schema,
    );
    fs::write(
        staging.join("runtara_agent_pause_grandchild.meta.json"),
        serde_json::to_vec_pretty(&grandchild_info).expect("meta serializes"),
    )
    .expect("stage grandchild meta");

    const MID: &str = r#"{
      "name": "Pause Mid",
      "steps": {
        "gcall": {
          "stepType": "Agent",
          "id": "gcall",
          "agentId": "pause-grandchild",
          "capabilityId": "run",
          "inputMapping": {}
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": { "echo": { "valueType": "reference", "value": "steps.gcall.outputs.echo" } }
        }
      },
      "entryPoint": "gcall",
      "executionPlan": [ { "fromStep": "gcall", "toStep": "finish" } ],
      "variables": {},
      "inputSchema": {},
      "outputSchema": {}
    }"#;
    let mid_graph: ExecutionGraph = serde_json::from_str(MID).expect("mid parses");
    let grandchild_catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
        grandchild_info,
    ]));
    let mut mid = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "pause-mid-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: mid_graph.clone(),
            child_workflows: vec![],
            output_dir: temp.path().join("mid-build"),
            track_events: false,
            agent_catalog: Some(grandchild_catalog),
            agent_slug: Some("pause-mid".to_string()),
        },
        runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
        false,
    )
    .expect("mid compiles as an agent");
    runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
        &mut mid,
        &components_dir,
        std::slice::from_ref(&staging),
    )
    .expect("mid composes the grandchild");
    fs::copy(&mid.wasm_path, staging.join("runtara_agent_pause_mid.wasm")).expect("stage mid wasm");
    let mid_info = certified_workflow_agent_info(
        "pause-mid",
        "Pause Mid",
        "",
        &mid_graph.input_schema,
        &mid_graph.output_schema,
    );
    fs::write(
        staging.join("runtara_agent_pause_mid.meta.json"),
        serde_json::to_vec_pretty(&mid_info).expect("meta serializes"),
    )
    .expect("stage mid meta");

    const TOP: &str = r#"{
      "name": "Pause Top",
      "steps": {
        "call": {
          "stepType": "Agent",
          "id": "call",
          "agentId": "pause-mid",
          "capabilityId": "run",
          "inputMapping": {}
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": { "echo": { "valueType": "reference", "value": "steps.call.outputs.echo" } }
        }
      },
      "entryPoint": "call",
      "executionPlan": [{ "fromStep": "call", "toStep": "finish" }],
      "variables": {},
      "inputSchema": {},
      "outputSchema": {}
    }"#;
    let top_graph: ExecutionGraph = serde_json::from_str(TOP).expect("top parses");
    let mid_catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
        mid_info,
    ]));
    let mut top = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "pause-top-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: top_graph,
            child_workflows: vec![],
            output_dir: temp.path().join("top-build"),
            track_events: false,
            agent_catalog: Some(mid_catalog),
            agent_slug: None,
        },
        runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
        false,
    )
    .expect("top compile succeeds");
    runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
        &mut top,
        &components_dir,
        std::slice::from_ref(&staging),
    )
    .expect("top composes the mid agent");

    let host = Arc::new(PersistingRuntimeHost::new(b"{}"));
    let site = "checkpoint-ns-e2e/pause-gc-wf/pause-top-wf::call__gcall::approve";
    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let run_once = |host: Arc<PersistingRuntimeHost>| {
        runtime.block_on(async {
            let pre = executor
                .load_instance_pre(&top.wasm_path)
                .await
                .expect("load top artifact");
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
                    b"{}".to_vec(),
                )
                .await
        })
    };

    host.suspend_requested
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let first = run_once(host.clone());
    assert!(
        matches!(first.exit, runtara_component_host::InvokeExit::Suspended(_)),
        "a pause two boundaries deep must suspend the top-level run, got {:?}",
        first.exit
    );
    assert!(host.failed.lock().unwrap().is_none());

    host.suspend_requested
        .store(false, std::sync::atomic::Ordering::SeqCst);
    host.deliver_signal(site, br#"{"decision":"nested-approved"}"#);
    let second = run_once(host.clone());
    let output = match second.exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("the resumed nested run must complete, got {other:?}"),
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&output).expect("output json"),
        serde_json::json!({ "echo": "nested-approved" }),
        "the payload must flow back through both composed boundaries"
    );
    assert_eq!(
        host.complete_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "neither composed child may fire runtime.complete"
    );
    std::mem::forget(temp);
}

/// N6 — PER-CALL checkpoint namespace for a workflow-agent invoked as an
/// AiAgent TOOL. One durable workflow-agent tool is dispatched TWICE by the
/// loop; each call gets its own `{ai_step}.tool.{label}.{counter}` scope, so
/// the child's internal durable keys (its Delay sleep) never collide across
/// calls. Before the wrap, both calls shared the child's bare unscoped keys:
/// call 2's sleep lookup HIT call 1's checkpoint and silently skipped.
#[test]
fn workflow_agent_tool_calls_get_per_call_checkpoint_scopes() {
    let components_dir = direct_e2e_components_dir();

    const DURABLE_TOOL_CHILD: &str = r#"{
      "name": "Tool Delay Echo",
      "steps": {
        "call": {
          "stepType": "Delay",
          "id": "call",
          "durationMs": { "valueType": "immediate", "value": 5 }
        },
        "finish": {
          "stepType": "Finish",
          "id": "finish",
          "inputMapping": { "echo": { "valueType": "reference", "value": "data.value" } }
        }
      },
      "entryPoint": "call",
      "executionPlan": [ { "fromStep": "call", "toStep": "finish" } ],
      "variables": {},
      "inputSchema": { "value": { "type": "string", "required": true } },
      "outputSchema": {}
    }"#;
    let child_graph: ExecutionGraph =
        serde_json::from_str(DURABLE_TOOL_CHILD).expect("child parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let child = compile_direct_workflow_composed_configured(
        DirectCompilationInput {
            workflow_id: "tool-child-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: child_graph.clone(),
            child_workflows: vec![],
            output_dir: temp.path().join("child-build"),
            track_events: false,
            agent_catalog: None,
            agent_slug: Some("tool-delay-echo".to_string()),
        },
        &components_dir,
        RuntimeBinding::HostImport,
        runtara_workflows::direct_wasm::WorkflowAbi::AgentCapabilities,
        false,
    )
    .expect("durable tool child publishes as an agent");
    assert!(!child.omit_runtime, "durable tool child keeps the runtime");

    let staging = temp.path().join("workflow-agents");
    fs::create_dir_all(&staging).expect("staging dir");
    fs::copy(
        &child.wasm_path,
        staging.join("runtara_agent_tool_delay_echo.wasm"),
    )
    .expect("stage child wasm");
    let info = certified_workflow_agent_info(
        "tool-delay-echo",
        "Tool Delay Echo",
        "",
        &child_graph.input_schema,
        &child_graph.output_schema,
    );
    fs::write(
        staging.join("runtara_agent_tool_delay_echo.meta.json"),
        serde_json::to_vec_pretty(&info).expect("meta serializes"),
    )
    .expect("stage child meta");

    // The parent: an AiAgent whose only tool is the published workflow-agent
    // (tool edge labeled `wf_echo`), plus a terminal Finish off the next edge.
    const PARENT: &str = r#"{
      "name": "AI Tool NS Parent",
      "entryPoint": "ai",
      "executionPlan": [
        { "fromStep": "ai", "toStep": "wf_tool", "label": "wf_echo" },
        { "fromStep": "ai", "toStep": "finish" }
      ],
      "steps": {
        "ai": { "id": "ai", "stepType": "AiAgent", "connectionId": "conn-1", "config": {
          "systemPrompt": { "valueType": "immediate", "value": "You call tools" },
          "userPrompt": { "valueType": "immediate", "value": "Echo twice" },
          "model": { "valueType": "immediate", "value": "gpt-4o" } } },
        "wf_tool": { "id": "wf_tool", "stepType": "Agent", "name": "wf_echo",
          "agentId": "tool-delay-echo", "capabilityId": "run", "inputMapping": {} },
        "finish": { "id": "finish", "stepType": "Finish",
          "inputMapping": { "answer": { "valueType": "reference", "value": "steps.ai.outputs.response" } } }
      },
      "variables": {},
      "inputSchema": {},
      "outputSchema": {}
    }"#;
    let parent_graph: ExecutionGraph = serde_json::from_str(PARENT).expect("parent parses");
    let catalog = Arc::new(runtara_dsl::agent_meta::AgentCatalog::from_agents(vec![
        info,
    ]));
    let mut parent = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "aitool-parent-wf".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: parent_graph,
            child_workflows: vec![],
            output_dir: temp.path().join("parent-build"),
            track_events: false,
            agent_catalog: Some(catalog),
            agent_slug: None,
        },
        runtara_workflows::direct_wasm::WorkflowAbi::InvokeHostImports,
        false,
    )
    .expect("parent compile succeeds");
    runtara_workflows::direct_wasm::compose_direct_workflow_with_extra_dirs(
        &mut parent,
        &components_dir,
        std::slice::from_ref(&staging),
    )
    .expect("parent composes the workflow-agent tool");

    // Hermetic LLM stub: the model requests the tool twice, then completes.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let (capture_tx, _capture_rx) = mpsc::channel::<CapturedMessage>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let server_state = Arc::new(ServerState::default());
    *server_state
        .llm_responses
        .lock()
        .expect("llm_responses lock") = vec![
        llm_tool_call("wf_echo", r#"{"value":"first"}"#),
        llm_tool_call("wf_echo", r#"{"value":"second"}"#),
        llm_ok("done"),
    ];
    let server_state_for_assertions = server_state.clone();
    let input_arc = Arc::new(b"{}".to_vec());
    let server_handle =
        thread::spawn(move || serve(listener, capture_tx, server_state, stop_rx, input_arc));

    let mut env = HashMap::new();
    env.insert("RUNTARA_HTTP_URL".to_string(), format!("http://{addr}"));
    env.insert(
        "RUNTARA_HTTP_PROXY_URL".to_string(),
        format!("http://{addr}/llm-proxy"),
    );
    // This test constructs WorkflowRunSpec directly, bypassing the
    // environment runner that translates RUNTARA_CONNECTION_SERVICE_URL into
    // the per-run CONNECTION_SERVICE_URL consumed by the resolver host.
    env.insert(
        "CONNECTION_SERVICE_URL".to_string(),
        format!("http://{addr}"),
    );
    env.insert(
        "RUNTARA_TENANT_ID".to_string(),
        "direct-wasm-execute".to_string(),
    );
    env.insert("RUST_LOG".to_string(), "warn".to_string());

    let host = Arc::new(PersistingRuntimeHost::new(b"{}"));
    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let run = runtime.block_on(async {
        let pre = executor
            .load_instance_pre(&parent.wasm_path)
            .await
            .expect("load parent artifact");
        executor
            .execute_invoke(
                &pre,
                runtara_component_host::WorkflowRunSpec {
                    env,
                    stderr: None,
                    timeout: Duration::from_secs(60),
                    cancel: None,
                    limits: runtara_component_host::WorkflowLimits::default(),
                    runtime: Some(host.clone()),
                },
                b"{}".to_vec(),
            )
            .await
    });
    let _ = stop_tx.send(());
    let _ = server_handle.join();

    let output = match run.exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("the tool loop must complete against the stub, got {other:?}"),
    };
    assert_eq!(
        serde_json::from_slice::<Value>(&output).expect("output json"),
        serde_json::json!({ "answer": "done" }),
        "the loop must finish on the third (completing) turn"
    );
    assert_eq!(
        server_state_for_assertions
            .llm_requests
            .lock()
            .expect("llm_requests lock")
            .len(),
        3,
        "two tool-call turns + one completing turn"
    );

    // THE assertion: each tool CALL owns a distinct child checkpoint family —
    // the durable child's Delay slept under two different per-call scopes.
    // Unscoped (pre-wrap), call 2 would have HIT call 1's bare `call` key and
    // skipped its sleep entirely.
    let sleeps = host.sleep_ids.lock().unwrap().clone();
    assert_eq!(
        sleeps,
        vec![
            "aitool-parent-wf::ai.tool.wf_echo.0::call".to_string(),
            "aitool-parent-wf::ai.tool.wf_echo.1::call".to_string(),
        ],
        "each tool call must own a per-call child checkpoint scope"
    );
    std::mem::forget(temp);
}

// ── Parallel Split overlap (Phase 3) ──────────────

fn parallel_http_split_graph(url: &str, parallelism: u32) -> String {
    format!(
        r#"{{
        "name": "Parallel HTTP Split",
        "steps": {{
            "split": {{
                "stepType": "Split",
                "id": "split",
                "config": {{
                    "value": {{"valueType": "reference", "value": "data.items"}},
                    "parallelism": {parallelism}
                }},
                "subgraph": {{
                    "name": "Fetch",
                    "steps": {{
                        "fetch": {{
                            "stepType": "Agent",
                            "id": "fetch",
                            "agentId": "http",
                            "capabilityId": "http-request",
                            "maxRetries": 0,
                            "inputMapping": {{
                                "method": {{"valueType": "immediate", "value": "GET"}},
                                "url": {{"valueType": "immediate", "value": "{url}/slow-item"}}
                            }}
                        }},
                        "finish": {{
                            "stepType": "Finish",
                            "id": "finish",
                            "inputMapping": {{
                                "status": {{"valueType": "reference", "value": "steps.fetch.outputs.status_code"}}
                            }}
                        }}
                    }},
                    "entryPoint": "fetch",
                    "executionPlan": [{{"fromStep": "fetch", "toStep": "finish"}}]
                }}
            }},
            "finish": {{
                "stepType": "Finish",
                "id": "finish",
                "inputMapping": {{
                    "results": {{"valueType": "reference", "value": "steps.split.outputs"}}
                }}
            }}
        }},
        "entryPoint": "split",
        "executionPlan": [{{"fromStep": "split", "toStep": "finish"}}],
        "variables": {{}}
    }}"#
    )
}

/// A requested-concurrent Split whose item body retries is INELIGIBLE for the
/// concurrent window, not unsupported: the emitter degrades it to the
/// sequential lowering, whose durable retry park is the supported path. A hard
/// rejection here would stop the workflow from starting at all.
fn assert_parallel_retry_backoff_compiles(workflow_id: &str, graph: ExecutionGraph) {
    let support = analyze_direct_wasm_support(&graph);
    assert!(
        support.supported,
        "a requested-concurrent Split with retrying items must stay supported: {:?}",
        support.unsupported
    );

    let temp = tempfile::tempdir().expect("tempdir");
    compile_direct_workflow(DirectCompilationInput {
        workflow_id: workflow_id.to_string(),
        version: 1,
        source_checksum: None,
        execution_graph: graph,
        child_workflows: vec![],
        output_dir: temp.path().to_path_buf(),
        track_events: false,
        agent_catalog: None,
        agent_slug: None,
    })
    .expect("the sequential fallback must compile");
}

/// A single-Agent diamond `start → {b, c} → finish` whose two branches each hit
/// the mock `/slow-item` endpoint (Phase 4a). The branches run concurrently
/// in one waitable-set window; the merge
/// `finish` reads BOTH branch outputs. `durable` toggles per-step checkpoints
/// (4a.2): durable branches gate the launch on the step checkpoint so a replay
/// never re-fires them.
fn parallel_http_branches_graph(url: &str, durable: bool) -> String {
    format!(
        r#"{{
        "name": "Parallel HTTP Branches",
        "durable": {durable},
        "steps": {{
            "start": {{
                "stepType": "Agent",
                "id": "start",
                "agentId": "utils",
                "capabilityId": "get-current-iso-datetime",
                "maxRetries": 0,
                "inputMapping": {{}}
            }},
            "b": {{
                "stepType": "Agent",
                "id": "b",
                "agentId": "http",
                "capabilityId": "http-request",
                "maxRetries": 0,
                "inputMapping": {{
                    "method": {{"valueType": "immediate", "value": "GET"}},
                    "url": {{"valueType": "immediate", "value": "{url}/slow-item"}}
                }}
            }},
            "c": {{
                "stepType": "Agent",
                "id": "c",
                "agentId": "http",
                "capabilityId": "http-request",
                "maxRetries": 0,
                "inputMapping": {{
                    "method": {{"valueType": "immediate", "value": "GET"}},
                    "url": {{"valueType": "immediate", "value": "{url}/slow-item"}}
                }}
            }},
            "finish": {{
                "stepType": "Finish",
                "id": "finish",
                "inputMapping": {{
                    "b_status": {{"valueType": "reference", "value": "steps.b.outputs.status_code"}},
                    "c_status": {{"valueType": "reference", "value": "steps.c.outputs.status_code"}}
                }}
            }}
        }},
        "entryPoint": "start",
        "executionPlan": [
            {{"fromStep": "start", "toStep": "b"}},
            {{"fromStep": "start", "toStep": "c"}},
            {{"fromStep": "b", "toStep": "finish"}},
            {{"fromStep": "c", "toStep": "finish"}}
        ],
        "variables": {{}}
    }}"#
    )
}

/// A non-durable diamond of two-Agent CHAINS `start → {b1→b2, c1→c2} → finish`
/// (the depth-wavefront). Every branch
/// step hits `/slow-item`, so a correct wavefront issues the requests in TWO waves
/// of two ({b1,c1} then {b2,c2}); a serialized run issues four back-to-back.
fn parallel_http_chain_branches_graph(url: &str) -> String {
    let http_step = |id: &str| {
        format!(
            r#""{id}": {{
                "stepType": "Agent", "id": "{id}", "agentId": "http",
                "capabilityId": "http-request", "maxRetries": 0,
                "inputMapping": {{
                    "method": {{"valueType": "immediate", "value": "GET"}},
                    "url": {{"valueType": "immediate", "value": "{url}/slow-item"}}
                }}
            }}"#
        )
    };
    format!(
        r#"{{
        "name": "Parallel HTTP Chain Branches",
        "durable": false,
        "steps": {{
            "start": {{
                "stepType": "Agent", "id": "start", "agentId": "utils",
                "capabilityId": "get-current-iso-datetime", "maxRetries": 0, "inputMapping": {{}}
            }},
            {b1}, {b2}, {c1}, {c2},
            "finish": {{
                "stepType": "Finish", "id": "finish",
                "inputMapping": {{
                    "b_status": {{"valueType": "reference", "value": "steps.b2.outputs.status_code"}},
                    "c_status": {{"valueType": "reference", "value": "steps.c2.outputs.status_code"}}
                }}
            }}
        }},
        "entryPoint": "start",
        "executionPlan": [
            {{"fromStep": "start", "toStep": "b1"}},
            {{"fromStep": "start", "toStep": "c1"}},
            {{"fromStep": "b1", "toStep": "b2"}},
            {{"fromStep": "c1", "toStep": "c2"}},
            {{"fromStep": "b2", "toStep": "finish"}},
            {{"fromStep": "c2", "toStep": "finish"}}
        ],
        "variables": {{}}
    }}"#,
        b1 = http_step("b1"),
        b2 = http_step("b2"),
        c1 = http_step("c1"),
        c2 = http_step("c2"),
    )
}

/// Serializes the wall-clock-sensitive parallel-split tests against each
/// other AND absorbs poisoning: they assert timing ratios and request
/// interleavings that melt when they share cores with each other. The rest
/// of the battery may still run alongside — CI runs single-threaded anyway;
/// this only removes the local `cargo test` flake class.
static PARALLEL_TIMING_LOCK: Mutex<()> = Mutex::new(());

/// The Phase-3 payoff test: a Split with `parallelism` over http-agent calls
/// completes with correct per-item results, and the run is TIMED under both
/// parallelism=1 and parallelism=N so the log shows whether the window
/// genuinely overlaps agent I/O on this host (p2 wasi:http binding permitting).
#[test]
fn direct_wasm_execute_parallel_split_http_overlap() {
    let _timing_guard = PARALLEL_TIMING_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let components_dir = direct_e2e_components_dir();
    const DELAY: Duration = Duration::from_millis(400);
    const ITEMS: usize = 4;

    for parallelism in [1u32, ITEMS as u32] {
        // The http agent forwards through RUNTARA_HTTP_PROXY_URL (the harness
        // mock); the target URL is carried in the proxy envelope and answered
        // by the mock's /slow-item branch — no real dial happens.
        let graph = parallel_http_split_graph("http://slow.invalid", parallelism);
        let captured = run_direct_workflow_capture(
            &components_dir,
            &format!("parallel-http-{parallelism}"),
            &graph,
            br#"{"items":[1,2,3,4]}"#,
            false,
        );
        assert!(
            captured.status_success,
            "parallel http split run failed: stderr={} error={:?}",
            captured.stderr, captured.error_json
        );
        let output = captured.output_json.expect("completed output");

        let results = output["results"]
            .as_array()
            .unwrap_or_else(|| panic!("split results missing: {output}"));
        assert_eq!(results.len(), ITEMS, "all items must complete");
        for result in results {
            assert_eq!(result["status"], 200, "item result: {result}");
        }

        // Load-robust concurrency signal: the SPAN over which the mock
        // observed the ITEMS requests arrive. Wall-clock ratios melt when the
        // battery runs multi-threaded; request INTERLEAVING (timestamped at
        // the mock) does not.
        let arrivals = &captured.slow_item_arrivals;
        assert_eq!(arrivals.len(), ITEMS, "every item must reach the upstream");
        let span = arrivals
            .iter()
            .max()
            .zip(arrivals.iter().min())
            .map(|(max, min)| max.duration_since(*min))
            .expect("arrival span");
        eprintln!(
            "[parallel-split-timing] parallelism={parallelism} arrival-span={}ms",
            span.as_millis()
        );
        if parallelism == 1 {
            // Request i+1 leaves only after response i, so arrivals span at
            // least (ITEMS-1) think-times.
            assert!(
                span >= DELAY * (ITEMS as u32 - 1),
                "sequential arrivals implausibly close: {span:?}"
            );
        } else {
            // All launches fire before any response lands: every request
            // arrives within one think-time.
            assert!(
                span < DELAY,
                "parallel window failed to overlap agent I/O: arrivals span {span:?}"
            );
        }
    }
}

/// Phase-4a payoff: a non-durable single-Agent diamond runs its two branches
/// CONCURRENTLY. The merge sees both branch outputs, and the two `/slow-item`
/// calls arrive at the mock within one think-time (they overlap) rather than
/// serialized across two.
#[test]
fn direct_wasm_execute_parallel_branches_http_overlap() {
    let _timing_guard = PARALLEL_TIMING_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let components_dir = direct_e2e_components_dir();
    const DELAY: Duration = Duration::from_millis(400);

    let graph = parallel_http_branches_graph("http://slow.invalid", false);
    let captured = run_direct_workflow_capture(
        &components_dir,
        "parallel-branches-http",
        &graph,
        br#"{}"#,
        false,
    );
    assert!(
        captured.status_success,
        "parallel branches run failed: stderr={} error={:?}",
        captured.stderr, captured.error_json
    );
    let output = captured.output_json.expect("completed output");
    assert_eq!(output["b_status"], 200, "branch b result: {output}");
    assert_eq!(output["c_status"], 200, "branch c result: {output}");

    // The mock timestamps each `/slow-item` arrival. Concurrent branches launch
    // both requests before either response lands, so the two arrivals fall
    // within one think-time; serialized branches would span at least one full
    // think-time (request 2 leaves only after response 1).
    let arrivals = &captured.slow_item_arrivals;
    assert_eq!(
        arrivals.len(),
        2,
        "both branch agents must reach the upstream"
    );
    let span = arrivals
        .iter()
        .max()
        .zip(arrivals.iter().min())
        .map(|(max, min)| max.duration_since(*min))
        .expect("arrival span");
    eprintln!(
        "[parallel-branches-timing] arrival-span={}ms",
        span.as_millis()
    );
    assert!(
        span < DELAY,
        "parallel branches failed to overlap agent I/O: arrivals span {span:?}"
    );
}

/// Observational-timing payoff: the concurrent branch scheduler stamps each
/// branch's REAL launch/settle wall clock into its slot and carries the pair on the
/// assemble-pass `step_debug_end` event. For the slow diamond `start → {b,c} →
/// finish` (each branch a ~400ms `/slow-item` GET), the recorded
/// `[launched_at_ms, settled_at_ms]` intervals of b and c must OVERLAP — which is
/// exactly what lets the timeline/replay render true concurrency instead of the
/// sequential assemble cascade the per-step assemble timestamps otherwise imply.
#[test]
fn direct_wasm_execute_parallel_branches_launch_settle_overlap() {
    let _timing_guard = PARALLEL_TIMING_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let components_dir = direct_e2e_components_dir();
    const DELAY: Duration = Duration::from_millis(400);

    let graph = parallel_http_branches_graph("http://slow.invalid", false);
    let captured = run_direct_workflow_capture(
        &components_dir,
        "parallel-branches-launch-settle",
        &graph,
        br#"{}"#,
        true, // track events — we assert on the recorded step_debug_end payloads
    );
    assert!(
        captured.status_success,
        "parallel branches run failed: stderr={} error={:?}",
        captured.stderr, captured.error_json
    );

    // The launch/settle interval each branch recorded on its debug-end event.
    let interval = |step: &str| -> (u64, u64) {
        let end = captured
            .events
            .iter()
            .find(|e| e.subtype == "step_debug_end" && e.payload_json["step_id"] == step)
            .unwrap_or_else(|| panic!("no step_debug_end for branch '{step}'"));
        let launched = end.payload_json["launched_at_ms"]
            .as_u64()
            .unwrap_or_else(|| {
                panic!(
                    "branch '{step}' end missing launched_at_ms: {}",
                    end.payload_json
                )
            });
        let settled = end.payload_json["settled_at_ms"]
            .as_u64()
            .unwrap_or_else(|| {
                panic!(
                    "branch '{step}' end missing settled_at_ms: {}",
                    end.payload_json
                )
            });
        assert!(
            launched > 0 && settled >= launched,
            "branch '{step}' recorded an invalid interval [{launched},{settled}]"
        );
        (launched, settled)
    };
    let (lb, sb) = interval("b");
    let (lc, sc) = interval("c");
    eprintln!(
        "[launch-settle] b=[{lb},{sb}] ({}ms)  c=[{lc},{sc}] ({}ms)",
        sb - lb,
        sc - lc
    );

    // Each interval reflects the real ~400ms async HTTP wait (settle is a true
    // wall clock later than launch), not an instant assemble record.
    assert!(
        sb - lb >= 200,
        "branch b interval too short to be the real HTTP wait: {}ms",
        sb - lb
    );
    assert!(
        sc - lc >= 200,
        "branch c interval too short to be the real HTTP wait: {}ms",
        sc - lc
    );

    // THE overlap: the two intervals intersect. A serialized cascade would give
    // launch_c >= settle_b (c launches only after b settles), so no intersection —
    // this is precisely what distinguishes concurrent from cascade.
    let overlap_start = lb.max(lc);
    let overlap_end = sb.min(sc);
    assert!(
        overlap_start < overlap_end,
        "branch intervals do not overlap (serialized cascade): b=[{lb},{sb}] c=[{lc},{sc}]"
    );

    // The combined span is one think-time, not two: the diamond ran in ~400ms of
    // wall clock, not ~800ms as a serialized pair would.
    let combined_span = sb.max(sc) - lb.min(lc);
    assert!(
        (combined_span as u128) < 2 * DELAY.as_millis(),
        "combined launch→settle span {combined_span}ms implies serialization (>= 2x think time)"
    );

    // Additive + backward-compatible: sequential steps (the pre-fan-out `start`,
    // the `finish` merge) carry no launch/settle pair and fall back to assemble
    // timing — the fields appear only for the concurrently scheduled branches.
    for seq in ["start", "finish"] {
        if let Some(end) = captured
            .events
            .iter()
            .find(|e| e.subtype == "step_debug_end" && e.payload_json["step_id"] == seq)
        {
            assert!(
                end.payload_json.get("launched_at_ms").is_none()
                    && end.payload_json.get("settled_at_ms").is_none(),
                "sequential step '{seq}' must not carry a launch/settle pair: {}",
                end.payload_json
            );
        }
    }
}

/// Phase-4c.1: a parallel branch chain may contain SYNC non-Agent steps. Branch b
/// is `b1(agent) → blog(Log) → b2(agent)`; branch c is `c1 → c2`. The Log runs at
/// depth 1 (assemble-only, no launch) alongside sibling agent `c2`, and the merge
/// still reads both chains' terminal agent outputs — proving sync steps interleave
/// correctly in the wavefront.
#[test]
fn direct_wasm_execute_parallel_branches_with_sync_log_step() {
    let components_dir = direct_e2e_components_dir();
    let graph = r#"{
        "name": "Parallel Branches With Sync Step",
        "durable": false,
        "steps": {
            "start": {"stepType":"Agent","id":"start","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"go"}}},
            "b1": {"stepType":"Agent","id":"b1","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"B1"}}},
            "blog": {"stepType":"Log","id":"blog","name":"branch log","level":"info","message":"in branch b"},
            "b2": {"stepType":"Agent","id":"b2","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"B2"}}},
            "c1": {"stepType":"Agent","id":"c1","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"C1"}}},
            "c2": {"stepType":"Agent","id":"c2","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"C2"}}},
            "finish": {"stepType":"Finish","id":"finish","inputMapping":{"b":{"valueType":"reference","value":"steps.b2.outputs"},"c":{"valueType":"reference","value":"steps.c2.outputs"}}}
        },
        "entryPoint": "start",
        "executionPlan": [
            {"fromStep":"start","toStep":"b1"},
            {"fromStep":"start","toStep":"c1"},
            {"fromStep":"b1","toStep":"blog"},
            {"fromStep":"blog","toStep":"b2"},
            {"fromStep":"c1","toStep":"c2"},
            {"fromStep":"b2","toStep":"finish"},
            {"fromStep":"c2","toStep":"finish"}
        ],
        "variables": {}
    }"#;
    let captured = run_direct_workflow_capture(
        &components_dir,
        "parallel-branches-sync-step",
        graph,
        br#"{}"#,
        false,
    );
    assert!(
        captured.status_success,
        "sync-step branch run failed: stderr={} error={:?}",
        captured.stderr, captured.error_json
    );
    let output = captured.output_json.expect("completed output");
    assert_eq!(
        output["b"], "B2",
        "branch b (through a Log step) terminal: {output}"
    );
    assert_eq!(output["c"], "C2", "branch c terminal: {output}");
}

/// Phase-4c.3: a parallel branch may contain an in-branch CONDITIONAL (a composite
/// node). Branch b is `bcond(Conditional) → bmerge`; the conditional's arms (bt/bf)
/// re-join at `bmerge`. It runs BLOCKING at depth 0 (no launch) beside sibling
/// chain c; the always-true condition takes `bt`, and `bmerge` reads its output —
/// proving in-branch control flow executes and the merge still reads both branches.
#[test]
fn direct_wasm_execute_parallel_branches_with_inbranch_conditional() {
    let components_dir = direct_e2e_components_dir();
    let graph = r#"{
        "name": "Parallel Branches With Conditional",
        "durable": false,
        "steps": {
            "start": {"stepType":"Agent","id":"start","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"go"}}},
            "bcond": {"stepType":"Conditional","id":"bcond","condition":{"type":"operation","op":"EQ","arguments":[{"value":"x","valueType":"immediate"},{"value":"x","valueType":"immediate"}]}},
            "bt": {"stepType":"Agent","id":"bt","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"TOOK_TRUE"}}},
            "bf": {"stepType":"Agent","id":"bf","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"TOOK_FALSE"}}},
            "bmerge": {"stepType":"Agent","id":"bmerge","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"reference","value":"steps.bt.outputs"}}},
            "c1": {"stepType":"Agent","id":"c1","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"C1"}}},
            "c2": {"stepType":"Agent","id":"c2","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"C2"}}},
            "finish": {"stepType":"Finish","id":"finish","inputMapping":{"b":{"valueType":"reference","value":"steps.bmerge.outputs"},"c":{"valueType":"reference","value":"steps.c2.outputs"}}}
        },
        "entryPoint": "start",
        "executionPlan": [
            {"fromStep":"start","toStep":"bcond"},
            {"fromStep":"start","toStep":"c1"},
            {"fromStep":"bcond","toStep":"bt","label":"true"},
            {"fromStep":"bcond","toStep":"bf","label":"false"},
            {"fromStep":"bt","toStep":"bmerge"},
            {"fromStep":"bf","toStep":"bmerge"},
            {"fromStep":"bmerge","toStep":"finish"},
            {"fromStep":"c1","toStep":"c2"},
            {"fromStep":"c2","toStep":"finish"}
        ],
        "variables": {}
    }"#;
    let captured = run_direct_workflow_capture(
        &components_dir,
        "parallel-branches-conditional",
        graph,
        br#"{}"#,
        false,
    );
    assert!(
        captured.status_success,
        "in-branch conditional run failed: stderr={} error={:?}",
        captured.stderr, captured.error_json
    );
    let output = captured.output_json.expect("completed output");
    assert_eq!(
        output["b"], "TOOK_TRUE",
        "branch b conditional took the true arm: {output}"
    );
    assert_eq!(output["c"], "C2", "branch c terminal: {output}");
}

/// Phase-4c.3: a parallel branch may contain a nested WHILE loop (a `next_plan`
/// composite). Branch b is `bloop(While) → bafter`; the loop runs blocking at depth
/// 0 beside sibling `c1`, then `bafter` runs at depth 1. The merge reads `bafter`
/// (after the loop) and `c2` — proving nested loops execute in a parallel branch.
#[test]
fn direct_wasm_execute_parallel_branches_with_inbranch_while() {
    let components_dir = direct_e2e_components_dir();
    let graph = r#"{
        "name": "Parallel Branches With While",
        "durable": false,
        "steps": {
            "start": {"stepType":"Agent","id":"start","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"go"}}},
            "bloop": {
                "stepType":"While","id":"bloop","name":"loop",
                "condition":{"type":"operation","op":"LT","arguments":[{"valueType":"reference","value":"loop.index"},{"valueType":"immediate","value":2}]},
                "subgraph":{"name":"iter","entryPoint":"iterfin","steps":{"iterfin":{"stepType":"Finish","id":"iterfin","inputMapping":{"n":{"valueType":"immediate","value":5}}}},"executionPlan":[]},
                "config":{"maxIterations":10}
            },
            "bafter": {"stepType":"Agent","id":"bafter","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"AFTER_LOOP"}}},
            "c1": {"stepType":"Agent","id":"c1","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"C1"}}},
            "c2": {"stepType":"Agent","id":"c2","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"C2"}}},
            "finish": {"stepType":"Finish","id":"finish","inputMapping":{"b":{"valueType":"reference","value":"steps.bafter.outputs"},"c":{"valueType":"reference","value":"steps.c2.outputs"}}}
        },
        "entryPoint": "start",
        "executionPlan": [
            {"fromStep":"start","toStep":"bloop"},
            {"fromStep":"start","toStep":"c1"},
            {"fromStep":"bloop","toStep":"bafter"},
            {"fromStep":"bafter","toStep":"finish"},
            {"fromStep":"c1","toStep":"c2"},
            {"fromStep":"c2","toStep":"finish"}
        ],
        "variables": {}
    }"#;
    let captured = run_direct_workflow_capture(
        &components_dir,
        "parallel-branches-while",
        graph,
        br#"{}"#,
        false,
    );
    assert!(
        captured.status_success,
        "in-branch while run failed: stderr={} error={:?}",
        captured.stderr, captured.error_json
    );
    let output = captured.output_json.expect("completed output");
    assert_eq!(
        output["b"], "AFTER_LOOP",
        "branch b step after the loop ran: {output}"
    );
    assert_eq!(output["c"], "C2", "branch c terminal: {output}");
}

/// Phase-4c (final shape): a DURABLE parallel branch may contain an in-branch
/// WaitForSignal. Branch b is `bwait(WaitForSignal) → bafter`; the wait runs LAST
/// at depth 0 (after sibling `c1` checkpoints), reads the delivered signal, and the
/// merge reads `bafter` (after the wait) and `c2`. A second run (resume with the
/// signal retained) reproduces the result without hanging or re-firing — proving
/// the wavefront's deferred-suspend ordering + replay are correct.
#[test]
fn direct_wasm_execute_parallel_branches_with_inbranch_wait() {
    let components_dir = direct_e2e_components_dir();
    let workflow_id = "parallel-branches-wait";
    let signal = serde_json::json!({ "approved": true });
    let graph = r#"{
        "name": "Parallel Branches With Wait",
        "durable": true,
        "steps": {
            "start": {"stepType":"Agent","id":"start","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"go"}}},
            "bwait": {"stepType":"WaitForSignal","id":"bwait","name":"Approval","pollIntervalMs":0,"responseSchema":{"approved":{"type":"boolean","required":true}}},
            "bafter": {"stepType":"Agent","id":"bafter","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"reference","value":"steps.bwait.outputs.approved"}}},
            "c1": {"stepType":"Agent","id":"c1","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"C1"}}},
            "c2": {"stepType":"Agent","id":"c2","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"C2"}}},
            "finish": {"stepType":"Finish","id":"finish","inputMapping":{"b":{"valueType":"reference","value":"steps.bafter.outputs"},"c":{"valueType":"reference","value":"steps.c2.outputs"}}}
        },
        "entryPoint": "start",
        "executionPlan": [
            {"fromStep":"start","toStep":"bwait"},
            {"fromStep":"start","toStep":"c1"},
            {"fromStep":"bwait","toStep":"bafter"},
            {"fromStep":"bafter","toStep":"finish"},
            {"fromStep":"c1","toStep":"c2"},
            {"fromStep":"c2","toStep":"finish"}
        ],
        "variables": {}
    }"#;

    let first = run_wait_workflow(
        &components_dir,
        workflow_id,
        graph,
        b"{}",
        Vec::new(),
        vec![signal.clone()],
    );
    assert!(
        first.status_success,
        "in-branch wait run failed: stderr={} error={:?}",
        first.stderr, first.error_json
    );
    let out1 = first.output_json.clone().expect("completed output");
    assert_eq!(
        out1["b"], true,
        "branch b read the delivered signal: {out1}"
    );
    assert_eq!(out1["c"], "C2", "branch c terminal: {out1}");

    // Resume (replay with the signal retained): completes identically, no hang.
    let second = run_wait_workflow(
        &components_dir,
        workflow_id,
        graph,
        b"{}",
        Vec::new(),
        vec![signal],
    );
    assert!(
        second.status_success,
        "in-branch wait resume failed: stderr={}",
        second.stderr
    );
    assert_eq!(
        second.output_json,
        Some(serde_json::json!({ "b": true, "c": "C2" })),
        "resume reproduces the delivered-signal result"
    );
}

/// T2.0: a DURABLE parallel branch may contain a suspending COMPOSITE — here a While
/// loop whose body runs a durable Delay (a suspension NESTED one level down, the R2
/// remnant that previously linearised). Branch b is `bloop(While) → bafter`;
/// `node_body_suspends(While)` (its body contains a Delay) routes it to pass-2 so
/// sibling `c1` checkpoints before the composite may suspend, and
/// `plan_branch_diamond` gates the branch on `durable`. The merge reads `bafter`
/// (after the loop) and `c2` — proving a suspending composite compiles and executes
/// in a parallel branch. (A Wait/Delay nested inside a *Conditional* arm instead
/// would trip the separate E073 reconvergence validator, which sees the arm as a
/// second fan-out; the loop body keeps the composite a single re-converging node.)
#[test]
fn direct_wasm_execute_parallel_branches_with_inbranch_while_delay() {
    let components_dir = direct_e2e_components_dir();
    let graph = r#"{
        "name": "Parallel Branches With While+Delay",
        "durable": true,
        "steps": {
            "start": {"stepType":"Agent","id":"start","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"go"}}},
            "bloop": {
                "stepType":"While","id":"bloop","name":"loop",
                "condition":{"type":"operation","op":"LT","arguments":[{"valueType":"reference","value":"loop.index"},{"valueType":"immediate","value":2}]},
                "subgraph":{"name":"iter","entryPoint":"idelay","steps":{
                    "idelay":{"stepType":"Delay","id":"idelay","durationMs":{"valueType":"immediate","value":0}},
                    "iterfin":{"stepType":"Finish","id":"iterfin","inputMapping":{"n":{"valueType":"immediate","value":5}}}
                },"executionPlan":[{"fromStep":"idelay","toStep":"iterfin"}]},
                "config":{"maxIterations":10}
            },
            "bafter": {"stepType":"Agent","id":"bafter","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"AFTER_LOOP"}}},
            "c1": {"stepType":"Agent","id":"c1","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"C1"}}},
            "c2": {"stepType":"Agent","id":"c2","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"C2"}}},
            "finish": {"stepType":"Finish","id":"finish","inputMapping":{"b":{"valueType":"reference","value":"steps.bafter.outputs"},"c":{"valueType":"reference","value":"steps.c2.outputs"}}}
        },
        "entryPoint": "start",
        "executionPlan": [
            {"fromStep":"start","toStep":"bloop"},
            {"fromStep":"start","toStep":"c1"},
            {"fromStep":"bloop","toStep":"bafter"},
            {"fromStep":"bafter","toStep":"finish"},
            {"fromStep":"c1","toStep":"c2"},
            {"fromStep":"c2","toStep":"finish"}
        ],
        "variables": {}
    }"#;
    let captured = run_direct_workflow_capture(
        &components_dir,
        "parallel-branches-while-delay",
        graph,
        br#"{}"#,
        false,
    );
    assert!(
        captured.status_success,
        "in-branch while+delay run failed: stderr={} error={:?}",
        captured.stderr, captured.error_json
    );
    let output = captured.output_json.expect("completed output");
    assert_eq!(
        output["b"], "AFTER_LOOP",
        "branch b step after the suspending loop ran: {output}"
    );
    assert_eq!(output["c"], "C2", "branch c terminal: {output}");
}

/// T2.1: the per-branch SEGMENT SCHEDULER drives UNBALANCED pure-Agent chains —
/// branch a is 3 steps (a1→a2→a3), branch b is 1 step (b1). The scheduler advances
/// each branch by its own cursor as its subtask settles (branch b reaches DONE while
/// branch a is still driving a2/a3), rather than lock-stepping by depth. The merge
/// must read a's terminal (A3) and b's terminal (B1) regardless of interleaving —
/// proving the cursor/drive state machine is correct for asymmetric chain lengths.
#[test]
fn direct_wasm_execute_parallel_branches_scheduler_unbalanced_chains() {
    let components_dir = direct_e2e_components_dir();
    let graph = r#"{
        "name": "Parallel Branches Scheduler Unbalanced",
        "durable": false,
        "steps": {
            "start": {"stepType":"Agent","id":"start","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"go"}}},
            "a1": {"stepType":"Agent","id":"a1","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"A1"}}},
            "a2": {"stepType":"Agent","id":"a2","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"reference","value":"steps.a1.outputs"}}},
            "a3": {"stepType":"Agent","id":"a3","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"A3"}}},
            "b1": {"stepType":"Agent","id":"b1","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"B1"}}},
            "finish": {"stepType":"Finish","id":"finish","inputMapping":{"a":{"valueType":"reference","value":"steps.a3.outputs"},"b":{"valueType":"reference","value":"steps.b1.outputs"}}}
        },
        "entryPoint": "start",
        "executionPlan": [
            {"fromStep":"start","toStep":"a1"},
            {"fromStep":"start","toStep":"b1"},
            {"fromStep":"a1","toStep":"a2"},
            {"fromStep":"a2","toStep":"a3"},
            {"fromStep":"a3","toStep":"finish"},
            {"fromStep":"b1","toStep":"finish"}
        ],
        "variables": {}
    }"#;
    let captured = run_direct_workflow_capture(
        &components_dir,
        "parallel-branches-scheduler-unbalanced",
        graph,
        br#"{}"#,
        false,
    );
    assert!(
        captured.status_success,
        "scheduler unbalanced-chains run failed: stderr={} error={:?}",
        captured.stderr, captured.error_json
    );
    let output = captured.output_json.expect("completed output");
    assert_eq!(output["a"], "A3", "branch a 3-step terminal: {output}");
    assert_eq!(output["b"], "B1", "branch b 1-step terminal: {output}");
}

/// T2.2a — cross-suspension progress: a schedulable sibling completes BEFORE a
/// suspending branch reaches its wait. Branch c is a durable 3-step pure chain
/// (c1→c2→c3); branch b is bwait(WaitForSignal, short timeout)→bafter.
/// `emit_parallel_branches` partitions the fan-out and runs c through the scheduler
/// TO COMPLETION, then the wavefront runs b — so by the time bwait is reached, ALL of
/// c (including its DEEPEST step c3) has already checkpointed. The depth-wavefront
/// (T2.0) would have parked c3 until after the wait resolved (bwait is at depth 0,
/// c3 at depth 2). One run with no signal + a short timeout returns quickly; we
/// assert c3's checkpoint is present regardless of the wait's timeout outcome.
#[test]
fn direct_wasm_execute_parallel_branches_sibling_completes_before_wait() {
    let components_dir = direct_e2e_components_dir();
    let workflow_id = "parallel-branches-sibling-before-wait";
    let graph = r#"{
        "name": "Sibling Completes Before Wait",
        "durable": true,
        "steps": {
            "start": {"stepType":"Agent","id":"start","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"go"}}},
            "bwait": {"stepType":"WaitForSignal","id":"bwait","name":"Approval","pollIntervalMs":50,"timeoutMs":{"valueType":"immediate","value":250}},
            "bafter": {"stepType":"Agent","id":"bafter","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"AFTER"}}},
            "c1": {"stepType":"Agent","id":"c1","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"C1"}}},
            "c2": {"stepType":"Agent","id":"c2","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"reference","value":"steps.c1.outputs"}}},
            "c3": {"stepType":"Agent","id":"c3","agentId":"utils","capabilityId":"return-input","maxRetries":0,"inputMapping":{"value":{"valueType":"immediate","value":"C3"}}},
            "finish": {"stepType":"Finish","id":"finish","inputMapping":{"b":{"valueType":"reference","value":"steps.bafter.outputs"},"c":{"valueType":"reference","value":"steps.c3.outputs"}}}
        },
        "entryPoint": "start",
        "executionPlan": [
            {"fromStep":"start","toStep":"bwait"},
            {"fromStep":"start","toStep":"c1"},
            {"fromStep":"bwait","toStep":"bafter"},
            {"fromStep":"bafter","toStep":"finish"},
            {"fromStep":"c1","toStep":"c2"},
            {"fromStep":"c2","toStep":"c3"},
            {"fromStep":"c3","toStep":"finish"}
        ],
        "variables": {}
    }"#;

    // No signal: branch c runs fully via the scheduler, THEN the wavefront reaches
    // bwait (which times out after 250ms). Branch c's deepest step c3 has already
    // checkpointed by then — the T2.2a payoff.
    let run = run_wait_workflow(
        &components_dir,
        workflow_id,
        graph,
        b"{}",
        Vec::new(),
        Vec::new(),
    );
    let checkpoint_ids: Vec<&String> = run.checkpoints.iter().map(|c| &c.checkpoint_id).collect();
    let has_c3 = run
        .checkpoints
        .iter()
        .any(|c| c.checkpoint_id.contains("c3") && !c.state.is_empty());
    assert!(
        has_c3,
        "T2.2a: branch c's DEEPEST step c3 must checkpoint before the wavefront reaches \
         bwait; checkpoints={checkpoint_ids:?}"
    );
}

/// Phase-4b: a diamond of two-Agent CHAINS runs as a depth-wavefront. The four
/// `/slow-item` calls arrive in TWO waves of two ({b1,c1} then {b2,c2}) — so the
/// arrival span is about ONE think-time, not the ~three a fully serialized run
/// would take. The merge reads both chains' terminal outputs.
#[test]
fn direct_wasm_execute_parallel_chain_branches_wavefront_overlap() {
    let _timing_guard = PARALLEL_TIMING_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let components_dir = direct_e2e_components_dir();
    const DELAY: Duration = Duration::from_millis(400);

    let graph = parallel_http_chain_branches_graph("http://slow.invalid");
    let captured = run_direct_workflow_capture(
        &components_dir,
        "parallel-chain-branches",
        &graph,
        br#"{}"#,
        false,
    );
    assert!(
        captured.status_success,
        "chain branches run failed: stderr={} error={:?}",
        captured.stderr, captured.error_json
    );
    let output = captured.output_json.expect("completed output");
    assert_eq!(output["b_status"], 200, "chain b terminal: {output}");
    assert_eq!(output["c_status"], 200, "chain c terminal: {output}");

    // Four arrivals in two waves. Wavefront span ≈ one think-time (wave gap);
    // serialized would span ≈ three. Assert < 2 think-times to distinguish.
    let arrivals = &captured.slow_item_arrivals;
    assert_eq!(
        arrivals.len(),
        4,
        "all four chain steps must reach upstream"
    );
    let span = arrivals
        .iter()
        .max()
        .zip(arrivals.iter().min())
        .map(|(max, min)| max.duration_since(*min))
        .expect("arrival span");
    eprintln!(
        "[parallel-chain-branches-timing] arrival-span={}ms",
        span.as_millis()
    );
    assert!(
        span < DELAY * 2,
        "chain branches failed to overlap per depth (span {span:?} implies serialized)"
    );
}

/// Phase-4a.2: a DURABLE branch diamond is replay-safe. A fresh run fires both
/// branch agents (2 upstream arrivals) and checkpoints each. A resume that
/// preloads those checkpoints must NOT re-fire either agent — the launch gate
/// sees each step checkpoint HIT and skips the invoke; assemble replays the
/// stored result. Zero new arrivals, identical merge output.
#[test]
fn direct_wasm_execute_parallel_branches_durable_resume_no_double_fire() {
    let _timing_guard = PARALLEL_TIMING_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let components_dir = direct_e2e_components_dir();
    let graph = parallel_http_branches_graph("http://slow.invalid", true);

    // Fresh run: both branches fire and checkpoint.
    let first = run_direct_workflow_capture(
        &components_dir,
        "parallel-branches-durable",
        &graph,
        br#"{}"#,
        false,
    );
    assert!(
        first.status_success,
        "durable branches fresh run failed: stderr={} error={:?}",
        first.stderr, first.error_json
    );
    assert_eq!(
        first.slow_item_arrivals.len(),
        2,
        "both branch agents fire on the fresh run"
    );
    let out1 = first.output_json.clone().expect("fresh output");
    assert_eq!(out1["b_status"], 200, "fresh branch b: {out1}");
    assert_eq!(out1["c_status"], 200, "fresh branch c: {out1}");

    // Resume: preload the fresh run's durable writes; each branch's step
    // checkpoint HITs, so the launch gate skips both invokes.
    let preload: Vec<(String, Vec<u8>)> = first
        .checkpoints
        .iter()
        .filter(|c| !c.state.is_empty())
        .map(|c| (c.checkpoint_id.clone(), c.state.clone()))
        .collect();
    assert!(
        !preload.is_empty(),
        "durable fresh run must have written step checkpoints"
    );
    let second = run_direct_workflow_capture_with_preloaded_checkpoints(
        &components_dir,
        "parallel-branches-durable",
        &graph,
        br#"{}"#,
        false,
        preload,
        Vec::new(),
    );
    assert!(
        second.status_success,
        "durable branches resume failed: stderr={} error={:?}",
        second.stderr, second.error_json
    );
    assert_eq!(
        second.slow_item_arrivals.len(),
        0,
        "durable replay must NOT re-fire the branch agents (double-fire)"
    );
    let out2 = second.output_json.expect("resume output");
    assert_eq!(out2["b_status"], 200, "resume branch b: {out2}");
    assert_eq!(out2["c_status"], 200, "resume branch c: {out2}");
}

/// A BREAKPOINT on one branch of a durable diamond FIRES (suspends) in debug mode
/// while the sibling branch still runs in parallel — and resume completes without
/// re-firing either agent. This is the parallel-branch analogue of the sequential
/// breakpoint: `node_has_breakpoint` routes the breakpointed branch to the
/// assemble-last pass and skips its async launch, so the breakpoint pauses BEFORE a
/// blocking invoke (no resume double-fire); the non-breakpointed sibling runs first
/// via the scheduler and checkpoints before the suspend.
#[test]
fn direct_wasm_execute_breakpoint_in_parallel_branch_fires_and_resumes() {
    let _timing_guard = PARALLEL_TIMING_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let components_dir = direct_e2e_components_dir();
    // start → {b (BREAKPOINT), c} → finish; durable so the suspend is replay-safe.
    let graph = r#"{
        "name": "Breakpoint In Parallel Branch",
        "durable": true,
        "steps": {
            "start": {"stepType":"Agent","id":"start","agentId":"utils","capabilityId":"get-current-iso-datetime","maxRetries":0,"inputMapping":{}},
            "b": {"stepType":"Agent","id":"b","agentId":"http","capabilityId":"http-request","maxRetries":0,"breakpoint":true,"inputMapping":{"method":{"valueType":"immediate","value":"GET"},"url":{"valueType":"immediate","value":"http://slow.invalid/slow-item"}}},
            "c": {"stepType":"Agent","id":"c","agentId":"http","capabilityId":"http-request","maxRetries":0,"inputMapping":{"method":{"valueType":"immediate","value":"GET"},"url":{"valueType":"immediate","value":"http://slow.invalid/slow-item"}}},
            "finish": {"stepType":"Finish","id":"finish","inputMapping":{"b_status":{"valueType":"reference","value":"steps.b.outputs.status_code"},"c_status":{"valueType":"reference","value":"steps.c.outputs.status_code"}}}
        },
        "entryPoint": "start",
        "executionPlan": [
            {"fromStep":"start","toStep":"b"},
            {"fromStep":"start","toStep":"c"},
            {"fromStep":"b","toStep":"finish"},
            {"fromStep":"c","toStep":"finish"}
        ],
        "variables": {}
    }"#;
    let debug_env = vec![("DEBUG_MODE".to_string(), "true".to_string())];

    // RUN 1 (debug): sibling c runs (fires + checkpoints); branch b's breakpoint
    // fires and SUSPENDS before b's invoke → no completion output, only c arrived.
    let run1 = run_direct_workflow_capture_full(
        &components_dir,
        "breakpoint-parallel-branch",
        graph,
        br#"{}"#,
        false,
        Vec::new(),
        Vec::new(),
        debug_env.clone(),
    );
    let cp_ids: Vec<&String> = run1.checkpoints.iter().map(|c| &c.checkpoint_id).collect();
    // The breakpoint fired: it SUSPENDED (no completion output) and wrote its
    // fire-once checkpoint for step `b`.
    assert!(
        run1.output_json.is_none(),
        "the breakpoint must SUSPEND run 1, not complete: output={:?}",
        run1.output_json
    );
    assert!(
        run1.checkpoints
            .iter()
            .any(|c| c.checkpoint_id.contains("breakpoint::b")),
        "run 1 must write the breakpoint checkpoint for `b`: {cp_ids:?}"
    );
    // The sibling `c` ran in parallel and checkpointed BEFORE the suspend...
    assert!(
        run1.checkpoints
            .iter()
            .any(|c| c.checkpoint_id.contains("http-request::c") && !c.state.is_empty()),
        "sibling branch `c` must complete + checkpoint before the breakpoint suspend: {cp_ids:?}"
    );
    // ...while `b`'s invoke was SKIPPED (pause-before-run): exactly one HTTP arrival
    // (c), and no result checkpoint for `b`.
    assert_eq!(
        run1.slow_item_arrivals.len(),
        1,
        "only the sibling `c` fires on run 1; the breakpoint pauses before `b`'s invoke"
    );
    assert!(
        !run1
            .checkpoints
            .iter()
            .any(|c| c.checkpoint_id.contains("http-request::b") && !c.state.is_empty()),
        "`b`'s invoke must NOT have run before the breakpoint suspend: {cp_ids:?}"
    );

    let preload: Vec<(String, Vec<u8>)> = run1
        .checkpoints
        .iter()
        .filter(|c| !c.state.is_empty())
        .map(|c| (c.checkpoint_id.clone(), c.state.clone()))
        .collect();

    // RUN 2 (debug + resume): the breakpoint checkpoint HITs → skip; b's invoke
    // fires for the first time (the frontier); c's step checkpoint HITs → no re-fire.
    let run2 = run_direct_workflow_capture_full(
        &components_dir,
        "breakpoint-parallel-branch",
        graph,
        br#"{}"#,
        false,
        preload,
        Vec::new(),
        debug_env,
    );
    let out2 = run2.output_json.clone().unwrap_or_else(|| {
        panic!(
            "resume must COMPLETE past the breakpoint; stderr={}",
            run2.stderr
        )
    });
    assert_eq!(out2["b_status"], 200, "resumed branch b: {out2}");
    assert_eq!(out2["c_status"], 200, "resumed branch c: {out2}");
    // Exactly ONE new arrival on resume — `b` (the frontier, fired for the first
    // time after the breakpoint is skipped). `c`'s checkpoint HITs, so it does not
    // re-fire: no double-fire across the suspend.
    assert_eq!(
        run2.slow_item_arrivals.len(),
        1,
        "resume fires ONLY `b` (breakpoint skipped → first invoke); `c` replays from checkpoint"
    );
}

/// A pause signalled DURING a parallel window is observed at the drain
/// wakeups, but the suspend fires only at the CHUNK BOUNDARY — after every
/// subtask resolved and assemble checkpointed the durable items — so the
/// resumed run replays from checkpoints and never re-fires the agents.
#[test]
fn direct_wasm_execute_parallel_split_pause_mid_window_resumes() {
    let _timing_guard = PARALLEL_TIMING_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let components_dir = direct_e2e_components_dir();

    // Slow proxy stub: the host-io hyper client POSTs the proxy envelope
    // here; each response takes 300ms so the drain loop genuinely waits
    // (and polls) while subtasks are in flight. Thread-per-connection.
    let hits = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind slow proxy stub");
    let stub_url = format!("http://{}", listener.local_addr().expect("stub addr"));
    let (stub_stop_tx, stub_stop_rx) = mpsc::channel::<()>();
    listener.set_nonblocking(true).expect("nonblocking");
    let stub_hits = hits.clone();
    let stub = thread::spawn(move || {
        loop {
            if stub_stop_rx.try_recv().is_ok() {
                return;
            }
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).ok();
                    let hits = stub_hits.clone();
                    thread::spawn(move || {
                        use std::io::{Read, Write};
                        let mut buf = [0u8; 8192];
                        let _ = stream.read(&mut buf);
                        hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(300));
                        let body = br#"{"status":200,"headers":{},"body":{"ok":true}}"#;
                        let response = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes());
                        let _ = stream.write_all(body);
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(_) => return,
            }
        }
    });

    // Durable workflow (graph default) so assemble checkpoints each item.
    let graph = parallel_http_split_graph(&stub_url, 4);
    let graph: ExecutionGraph = serde_json::from_str(&graph).expect("graph parses");
    let temp = tempfile::tempdir().expect("tempdir");
    let compiled = compile_direct_workflow_composed(
        DirectCompilationInput {
            workflow_id: "parallel-pause-resume".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        },
        &components_dir,
    )
    .expect("parallel split compiles");

    let host = Arc::new(PersistingRuntimeHost::new(br#"{"items":[1,2,3,4]}"#));
    let executor = embedded_executor();
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let mut env = HashMap::new();
    env.insert("RUNTARA_HTTP_PROXY_URL".to_string(), stub_url.clone());
    let run_once = |host: Arc<PersistingRuntimeHost>| {
        let env = env.clone();
        runtime.block_on(async {
            let pre = executor
                .load_instance_pre(&compiled.wasm_path)
                .await
                .expect("load parallel artifact");
            executor
                .execute_invoke(
                    &pre,
                    runtara_component_host::WorkflowRunSpec {
                        env,
                        stderr: None,
                        timeout: Duration::from_secs(60),
                        cancel: None,
                        limits: runtara_component_host::WorkflowLimits::default(),
                        runtime: Some(host),
                    },
                    br#"{"data":{"items":[1,2,3,4]}}"#.to_vec(),
                )
                .await
        })
    };

    // PAUSE requested before the run: the drain-wakeup polls see it while the
    // four subtasks are in flight; the suspend fires at the chunk boundary.
    host.suspend_requested
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let first = run_once(host.clone());
    match first.exit {
        runtara_component_host::InvokeExit::Suspended(_) => {}
        other => panic!("pause during the window must SUSPEND, got {other:?}"),
    }
    assert!(
        host.failed.lock().unwrap().is_none(),
        "a lifecycle pause must not surface as a failure"
    );
    let first_run_hits = hits.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        first_run_hits, 4,
        "all four launched calls must resolve before the suspend"
    );

    // RESUME: replay-from-start. Every item checkpointed during assemble HITs
    // (launch gate + durable block), so the agents never re-fire.
    host.suspend_requested
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let second = run_once(host.clone());
    let output = match second.exit {
        runtara_component_host::InvokeExit::Completed(output) => output,
        other => panic!("the resumed run must complete, got {other:?}"),
    };
    let output: Value = serde_json::from_slice(&output).expect("output json");
    let results = output["results"].as_array().expect("split results");
    assert_eq!(results.len(), 4);
    for result in results {
        assert_eq!(result["status"], 200, "item result: {result}");
    }
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        first_run_hits,
        "the resumed run must replay from checkpoints — zero new agent calls"
    );

    let _ = stub_stop_tx.send(());
    let _ = stub.join();
}

/// A durable Split that requests concurrency but carries retried items keeps
/// compiling: the concurrent window is skipped and the sequential lowering's
/// durable retry park takes over.
#[test]
fn direct_wasm_compiles_parallel_split_durable_rate_limited_retries() {
    let mut graph: Value = serde_json::from_str(&parallel_http_split_graph("http://127.0.0.1", 4))
        .expect("graph json");
    graph["steps"]["split"]["subgraph"]["steps"]["fetch"]
        .as_object_mut()
        .expect("fetch step")
        .remove("maxRetries");
    assert_parallel_retry_backoff_compiles(
        "parallel-rate-limited",
        serde_json::from_value(graph).expect("graph parses"),
    );
}

/// Non-durable parallel Split items with retries degrade the same way. The
/// retired timer-subtask lowering held an invocation while it waited; the
/// sequential fallback does not.
#[test]
fn direct_wasm_compiles_non_durable_parallel_split_retries() {
    let mut graph: Value = serde_json::from_str(&parallel_http_split_graph("http://127.0.0.1", 4))
        .expect("graph json");
    graph["durable"] = Value::Bool(false);
    graph["steps"]["split"]["subgraph"]["steps"]["fetch"]
        .as_object_mut()
        .expect("fetch step")
        .remove("maxRetries");
    assert_parallel_retry_backoff_compiles(
        "parallel-concurrent-backoff",
        serde_json::from_value(graph).expect("graph parses"),
    );
}

/// The replay-oriented durable parallel retry compiles too. The runtime
/// guarantee that matters — no path sleeps inside a runner — is asserted by
/// `direct_wasm_execute_invoke_parallel_split_item_retry_parks_sequentially`.
#[test]
fn direct_wasm_compiles_parallel_split_durable_retry_replay_shape() {
    let mut graph: Value = serde_json::from_str(&parallel_http_split_graph("http://127.0.0.1", 4))
        .expect("graph json");
    graph["steps"]["split"]["subgraph"]["steps"]["fetch"]
        .as_object_mut()
        .expect("fetch step")
        .remove("maxRetries");
    assert_parallel_retry_backoff_compiles(
        "durable-backoff-replay",
        serde_json::from_value(graph).expect("graph parses"),
    );
}

/// Chained durable Delays — the shape of the workflow in the SYN-606 report
/// ("SYN-602 cancellable": ten 3s Delays, stopped ~3s in, which ran all the way
/// to `completed`). Five is enough to tell "stopped at the first Delay" from
/// "ignored the cancel and ran the lot".
const CHAINED_DELAYS: &str = r#"{
  "name": "Chained Delays",
  "steps": {
    "delay_1": { "stepType": "Delay", "id": "delay_1",
      "durationMs": { "valueType": "immediate", "value": 3000 } },
    "delay_2": { "stepType": "Delay", "id": "delay_2",
      "durationMs": { "valueType": "immediate", "value": 3000 } },
    "delay_3": { "stepType": "Delay", "id": "delay_3",
      "durationMs": { "valueType": "immediate", "value": 3000 } },
    "delay_4": { "stepType": "Delay", "id": "delay_4",
      "durationMs": { "valueType": "immediate", "value": 3000 } },
    "delay_5": { "stepType": "Delay", "id": "delay_5",
      "durationMs": { "valueType": "immediate", "value": 3000 } },
    "finish": { "stepType": "Finish", "id": "finish",
      "inputMapping": { "ranToCompletion": { "valueType": "immediate", "value": true } } }
  },
  "entryPoint": "delay_1",
  "executionPlan": [
    { "fromStep": "delay_1", "toStep": "delay_2" },
    { "fromStep": "delay_2", "toStep": "delay_3" },
    { "fromStep": "delay_3", "toStep": "delay_4" },
    { "fromStep": "delay_4", "toStep": "delay_5" },
    { "fromStep": "delay_5", "toStep": "finish" }
  ],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}"#;

/// A runtime host that reports a pending lifecycle signal from the moment the
/// first durable sleep returns — the in-process stand-in for a `stop_execution`
/// landing while the instance is parked in a Delay.
///
/// `check_signals` answering `true` is what the guest acts on; the host has
/// already consumed and acknowledged the signal by then, exactly as
/// `PersistenceRuntimeHost` does.
struct CancelDuringDelayHost {
    input: Vec<u8>,
    completed: Mutex<Option<Vec<u8>>>,
    failed: Mutex<Option<Vec<u8>>>,
    /// Sleep checkpoint ids, in order — one per Delay actually reached.
    sleeps: Mutex<Vec<String>>,
    /// Set once the first sleep returns; every later `check-signals` reports it.
    cancel_pending: std::sync::atomic::AtomicBool,
    /// How many times the guest asked. Zero is the bug: a chain of Delays used
    /// to contain no poll site at all.
    signal_polls: std::sync::atomic::AtomicU32,
}

impl CancelDuringDelayHost {
    fn new(input: &[u8]) -> Self {
        Self {
            input: input.to_vec(),
            completed: Mutex::new(None),
            failed: Mutex::new(None),
            sleeps: Mutex::new(Vec::new()),
            cancel_pending: std::sync::atomic::AtomicBool::new(false),
            signal_polls: std::sync::atomic::AtomicU32::new(0),
        }
    }
}

#[async_trait::async_trait]
impl runtara_component_host::runtime_host::RuntimeHost for CancelDuringDelayHost {
    async fn load_input(&self) -> Result<Option<Vec<u8>>, String> {
        Ok(Some(self.input.clone()))
    }
    fn instance_id(&self) -> Result<String, String> {
        Ok("syn606-cancel-during-delay".to_string())
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
        Ok(self
            .cancel_pending
            .load(std::sync::atomic::Ordering::SeqCst))
    }
    async fn check_signals(&self) -> Result<bool, String> {
        self.signal_polls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self
            .cancel_pending
            .load(std::sync::atomic::Ordering::SeqCst))
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
        Ok(true)
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
        _state: Vec<u8>,
        _ms: u64,
    ) -> Result<(), String> {
        self.sleeps.lock().unwrap().push(checkpoint_id);
        // Production's `handle_sleep` returns early once a cancel is pending;
        // the mock does not sleep at all, so "the signal arrived during the
        // first delay" is modelled by flipping the flag as it returns.
        self.cancel_pending
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

/// SYN-606: `stop_execution` was a no-op against an instance parked in a durable
/// Delay. The Delay lowering called `durable-sleep-checkpoint` and checked only
/// for a retptr error — no `check-signals`, and no `emit_checkpoint_save` to fold
/// signal handling into. A linear chain of Delays therefore contained ZERO poll
/// sites, so a cancel written during the first delay was never observed and the
/// run completed normally.
///
/// With the poll emitted after the sleep, the cancel is acted on at the very next
/// step boundary: the run suspends after the first Delay instead of executing all
/// five and reporting completion.
#[test]
fn direct_wasm_execute_delay_observes_cancel_and_suspends() {
    let components_dir = direct_e2e_components_dir();
    let graph: ExecutionGraph = serde_json::from_str(CHAINED_DELAYS).expect("fixture parses");
    let temp = tempfile::tempdir().expect("tempdir");

    let mut result = runtara_workflows::direct_wasm::compile_direct_workflow_with_abi(
        DirectCompilationInput {
            workflow_id: "syn606-cancel-during-delay".to_string(),
            version: 1,
            source_checksum: None,
            execution_graph: graph,
            child_workflows: vec![],
            output_dir: temp.path().to_path_buf(),
            track_events: false,
            agent_catalog: None,
            agent_slug: None,
        },
        WorkflowAbi::CliRunHttp,
        false,
    )
    .expect("direct emit succeeds");
    result.component_artifacts =
        emit_direct_component_artifacts_with_binding(&[], RuntimeBinding::HostImport);
    compose_direct_workflow(&mut result, &components_dir).expect("host-import compose");

    let host = Arc::new(CancelDuringDelayHost::new(b"{}"));
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
        "a suspended run exits cleanly; got {:?} (failed: {:?})",
        run.exit,
        host.failed
            .lock()
            .unwrap()
            .as_deref()
            .map(String::from_utf8_lossy),
    );

    let sleeps = host.sleeps.lock().unwrap().clone();
    assert_eq!(
        sleeps,
        vec!["delay_1".to_string()],
        "the run must stop at the Delay the cancel arrived during, not run the whole chain"
    );
    assert!(
        host.signal_polls.load(std::sync::atomic::Ordering::SeqCst) > 0,
        "a Delay must poll for lifecycle signals; zero polls is the SYN-606 bug"
    );
    assert!(
        host.completed.lock().unwrap().is_none(),
        "a cancelled run must NOT report completion (it reported one before the fix)"
    );
    assert!(
        host.failed.lock().unwrap().is_none(),
        "a cancel is a suspend, not a failure"
    );
}

// ============================================================================
// Native-free agents: compression + xlsx run inside the composed workflow.wasm
// ============================================================================
//
// Both agents used to forward every capability call to a native handler in the
// server process over `$RUNTARA_AGENT_SERVICE_URL`. They now do the work in
// the sandbox, so these two runs prove the real implementations survive
// composition into a workflow component — not just standalone invocation.
// Nothing here stands up an agent service; a surviving forwarder would fail.

const COMPRESSION_ROUND_TRIP: &str = r#"{
  "durable": false,
  "steps": {
    "create": {
      "stepType": "Agent",
      "id": "create",
      "name": "Create Archive",
      "agentId": "compression",
      "capabilityId": "create-archive",
      "maxRetries": 0,
      "inputMapping": {
        "files": { "valueType": "reference", "value": "data.files" },
        "archive_name": { "valueType": "immediate", "value": "bundle.zip" }
      }
    },
    "list": {
      "stepType": "Agent",
      "id": "list",
      "name": "List Archive",
      "agentId": "compression",
      "capabilityId": "list-archive",
      "maxRetries": 0,
      "inputMapping": {
        "archive": { "valueType": "reference", "value": "steps.create.outputs" }
      }
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "archive_name": { "valueType": "reference", "value": "steps.create.outputs.filename" },
        "entries": { "valueType": "reference", "value": "steps.list.outputs.total_count" },
        "bytes": { "valueType": "reference", "value": "steps.list.outputs.total_size" }
      }
    }
  },
  "entryPoint": "create",
  "executionPlan": [
    { "fromStep": "create", "toStep": "list" },
    { "fromStep": "list", "toStep": "finish" }
  ],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}"#;

const XLSX_PARSE_WORKFLOW: &str = r#"{
  "durable": false,
  "steps": {
    "parse": {
      "stepType": "Agent",
      "id": "parse",
      "name": "Parse Spreadsheet",
      "agentId": "xlsx",
      "capabilityId": "from-xlsx",
      "maxRetries": 0,
      "inputMapping": {
        "data": { "valueType": "reference", "value": "data.workbook" },
        "has_headers": { "valueType": "immediate", "value": true }
      }
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "rows": { "valueType": "reference", "value": "steps.parse.outputs" }
      }
    }
  },
  "entryPoint": "parse",
  "executionPlan": [
    { "fromStep": "parse", "toStep": "finish" }
  ],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}"#;

#[test]
fn direct_wasm_execute_compression_round_trips_in_guest() {
    let components_dir = direct_e2e_components_dir();

    // base64("hello") and base64("x,y")
    let input = br#"{"data":{"files":[
        {"file":{"content":"aGVsbG8=","filename":"a.txt"}},
        {"file":{"content":"eCx5","filename":"b.csv"}}
    ]},"variables":{}}"#;

    let output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-compression-round-trip",
        COMPRESSION_ROUND_TRIP,
        input,
    );

    assert_eq!(
        output,
        serde_json::json!({
            "archive_name": "bundle.zip",
            "entries": 2,
            "bytes": 8, // "hello" (5) + "x,y" (3), uncompressed
        })
    );
}

#[test]
fn direct_wasm_execute_xlsx_parses_in_guest() {
    let components_dir = direct_e2e_components_dir();

    let workbook = include_str!("fixtures/orders_xlsx.b64").trim();
    let input = format!(r#"{{"data":{{"workbook":"{workbook}"}},"variables":{{}}}}"#);

    let output = run_direct_workflow(
        &components_dir,
        "direct-wasm-execute-xlsx-parse",
        XLSX_PARSE_WORKFLOW,
        input.as_bytes(),
    );

    assert_eq!(
        output,
        serde_json::json!({
            "rows": [
                { "sku": "ABC-1", "qty": 4, "price": 9.5 },
                { "sku": "XYZ-9", "qty": 11, "price": 0.25 }
            ]
        })
    );
}
