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
    ChildWorkflowInput, CompilationInput, DirectWorkflowCompileOptions, ExecutionGraph,
    WorkflowCompilerMode, compile_workflow, compile_workflow_direct,
};
use serde_json::Value;
use tempfile::TempDir;

const SIMPLE_PASSTHROUGH: &str = include_str!("fixtures/simple_passthrough.json");
const EMBED_WORKFLOW: &str = include_str!("fixtures/embed_workflow_workflow.json");
const EMBED_WORKFLOW_FINISH_CHILD: &str = include_str!("fixtures/embed_workflow_finish_child.json");
const EMBED_WORKFLOW_ERROR_CHILD: &str = include_str!("fixtures/embed_workflow_error_child.json");
const EMBED_WORKFLOW_TRANSIENT_ERROR_CHILD: &str =
    include_str!("fixtures/embed_workflow_transient_error_child.json");
const EMBED_WORKFLOW_RETRY_NESTED_CHILD: &str =
    include_str!("fixtures/embed_workflow_retry_nested_child.json");
const EMBED_WORKFLOW_TRANSIENT_ERROR_GRANDCHILD: &str =
    include_str!("fixtures/embed_workflow_transient_error_grandchild.json");
const EMBED_WORKFLOW_RETRY_PARENT: &str = include_str!("fixtures/embed_workflow_retry_parent.json");
const EMBED_WORKFLOW_NO_RETRY_PARENT: &str =
    include_str!("fixtures/embed_workflow_no_retry_parent.json");
const EMBED_WORKFLOW_RETRY_ON_ERROR_PARENT: &str =
    include_str!("fixtures/embed_workflow_retry_on_error_parent.json");
const EMBED_WORKFLOW_CHILD_LOCAL_ON_ERROR_PARENT: &str =
    include_str!("fixtures/embed_workflow_child_local_on_error_parent.json");
const EMBED_WORKFLOW_CHILD_LOCAL_ON_ERROR_CHILD: &str =
    include_str!("fixtures/embed_workflow_child_local_on_error_child.json");
const EMBED_WORKFLOW_CONDITIONAL_ERROR_CHILD: &str =
    include_str!("fixtures/embed_workflow_conditional_error_child.json");
const EMBED_WORKFLOW_ON_ERROR_PARENT: &str =
    include_str!("fixtures/embed_workflow_on_error_parent.json");
const EMBED_WORKFLOW_NESTED_PARENT: &str =
    include_str!("fixtures/embed_workflow_nested_parent.json");
const EMBED_WORKFLOW_NESTED_CHILD: &str = include_str!("fixtures/embed_workflow_nested_child.json");
const EMBED_WORKFLOW_NESTED_GRANDCHILD: &str =
    include_str!("fixtures/embed_workflow_nested_grandchild.json");
const EMBED_WORKFLOW_NESTED_GREAT_GRANDCHILD: &str =
    include_str!("fixtures/embed_workflow_nested_great_grandchild.json");
const EMBED_WORKFLOW_NESTED_ERROR_GREAT_GRANDCHILD: &str =
    include_str!("fixtures/embed_workflow_nested_error_great_grandchild.json");
const CONDITIONAL_WORKFLOW: &str = include_str!("fixtures/conditional_workflow.json");
const FILTER_SIMPLE: &str = include_str!("fixtures/filter_simple.json");
const SWITCH_VALUE_SIMPLE: &str = include_str!("fixtures/switch_value_simple.json");
const SWITCH_ROUTING_SIMPLE: &str = include_str!("fixtures/switch_routing_simple.json");
const GROUP_BY_SIMPLE: &str = include_str!("fixtures/group_by_simple.json");
const EDGE_CONDITION_PRIORITY: &str = include_str!("fixtures/edge_condition_priority.json");
const WHILE_DIRECT_INDEX_ONLY: &str = include_str!("fixtures/while_direct_index_only.json");
const SPLIT_NESTED_SPLIT: &str = include_str!("fixtures/split_nested_split.json");
const WHILE_NESTED_SPLIT: &str = include_str!("fixtures/while_nested_split.json");
const WHILE_ON_ERROR: &str = include_str!("fixtures/while_on_error.json");
const SPLIT_ON_ERROR: &str = include_str!("fixtures/split_on_error.json");
const AGENT_COMPENSATION: &str = include_str!("fixtures/agent_compensation.json");
const AI_AGENT_SINGLE_SHOT: &str = include_str!("fixtures/ai_agent_single_shot.json");
const AI_AGENT_STRUCTURED: &str = include_str!("fixtures/ai_agent_structured.json");
const AI_AGENT_TOOL_LOOP: &str = include_str!("fixtures/ai_agent_tool_loop.json");
const AI_AGENT_MULTI_TOOL: &str = include_str!("fixtures/ai_agent_multi_tool.json");
const AI_AGENT_MEMORY: &str = include_str!("fixtures/ai_agent_memory.json");
const AI_AGENT_MEMORY_COMPACTION: &str = include_str!("fixtures/ai_agent_memory_compaction.json");
const AI_AGENT_MEMORY_SUMMARIZE: &str = include_str!("fixtures/ai_agent_memory_summarize.json");
const AI_AGENT_MCP: &str = include_str!("fixtures/ai_agent_mcp.json");
const AI_AGENT_TOOL_ERROR: &str = include_str!("fixtures/ai_agent_tool_error.json");
const FANOUT_DIAMOND: &str = include_str!("fixtures/fanout_diamond.json");
/// Canned assistant text returned by the mock LLM proxy in `route`. It is valid
/// JSON so the same mock drives both the plain single-shot test (response is the
/// JSON string) and the structured-output test (response is the parsed object).
const MOCK_AI_RESPONSE: &str = "{\"sentiment\":\"positive\",\"confidence\":0.9}";
const SPLIT_DONT_STOP_NESTED_SPLIT_ERROR: &str =
    include_str!("fixtures/split_dont_stop_nested_split_error.json");
const SPLIT_DONT_STOP_DEEP_NESTED_WHILE_SPLIT_ERROR: &str =
    include_str!("fixtures/split_dont_stop_deep_nested_while_split_error.json");
const LOG_ALL_LEVELS: &str = include_str!("fixtures/log_all_levels.json");
const ERROR_DIRECT_SIMPLE: &str = include_str!("fixtures/error_direct_simple.json");
const DELAY_DYNAMIC: &str = include_str!("fixtures/delay_dynamic.json");
const WAIT_FOR_SIGNAL_DIRECT_SIMPLE: &str =
    include_str!("fixtures/wait_for_signal_direct_simple.json");
const WAIT_FOR_SIGNAL_DIRECT_TIMEOUT: &str =
    include_str!("fixtures/wait_for_signal_direct_timeout.json");
const WAIT_FOR_SIGNAL_DIRECT_ON_WAIT: &str =
    include_str!("fixtures/wait_for_signal_direct_on_wait.json");
const WAIT_FOR_SIGNAL_DIRECT_ON_WAIT_ERROR: &str =
    include_str!("fixtures/wait_for_signal_direct_on_wait_error.json");
const WAIT_FOR_SIGNAL_DIRECT_BREAKPOINT: &str = r#"{
  "name": "Wait for Signal Direct Breakpoint",
  "durable": true,
  "steps": {
    "wait": {
      "stepType": "WaitForSignal",
      "id": "wait",
      "name": "Approval",
      "breakpoint": true,
      "pollIntervalMs": 0,
      "responseSchema": {
        "approved": {
          "type": "boolean",
          "required": true
        }
      },
      "action": {
        "key": "approval_decision",
        "correlation": {
          "case_id": {
            "valueType": "reference",
            "value": "data.case_id"
          }
        },
        "context": {
          "summary": {
            "valueType": "reference",
            "value": "data.summary"
          }
        }
      }
    },
    "finish": {
      "stepType": "Finish",
      "id": "finish",
      "inputMapping": {
        "approved": {
          "valueType": "reference",
          "value": "steps.wait.outputs.approved"
        }
      }
    }
  },
  "entryPoint": "wait",
  "executionPlan": [
    {
      "fromStep": "wait",
      "toStep": "finish"
    }
  ],
  "variables": {},
  "inputSchema": {},
  "outputSchema": {}
}"#;
const AGENT_CACHE_KEY: &str = "agent::utils::return-input::agent";
const SPLIT_CACHE_KEY: &str = "split::split";
const EMBED_WORKFLOW_CACHE_KEY: &str = "embed_workflow::call_child";
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
const SPLIT_RETRY_TRANSIENT_ERROR: &str = r#"{
  "durable": true,
  "steps": {
    "split": {
      "stepType": "Split",
      "id": "split",
      "config": {
        "value": { "valueType": "reference", "value": "data.items" },
        "sequential": true,
        "maxRetries": 2,
        "retryDelay": 1
      },
      "subgraph": {
        "name": "Transient Item Failure",
        "steps": {
          "fail": {
            "stepType": "Error",
            "id": "fail",
            "name": "Transient Item Failure",
            "category": "transient",
            "code": "SPLIT_ITEM_TEMPORARY",
            "message": "Split item failed transiently",
            "severity": "error",
            "context": {
              "item": { "valueType": "reference", "value": "data.value" },
              "index": { "valueType": "reference", "value": "variables._index" }
            }
          }
        },
        "entryPoint": "fail",
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
struct RetryAttemptRequest {
    checkpoint_id: String,
    attempt: u32,
    error_json: Option<Value>,
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
    RetryAttempt(RetryAttemptRequest),
    Suspended,
    SignalAck(SignalAckRequest),
    /// The conversation messages an AiAgent's `save-memory` invoke persisted to
    /// the object-model provider (after sliding-window compaction).
    MemorySave(Vec<Value>),
}

#[derive(Debug)]
struct CapturedRun {
    output_json: Option<Value>,
    error_json: Option<Value>,
    events: Vec<RuntimeEvent>,
    sleeps: Vec<SleepRequest>,
    checkpoints: Vec<CheckpointRequest>,
    retry_attempts: Vec<RetryAttemptRequest>,
    suspended_count: usize,
    signal_acks: Vec<SignalAckRequest>,
    memory_saves: Vec<Vec<Value>>,
    status_success: bool,
    stderr: String,
}

struct ServerState {
    checkpoints: Mutex<HashMap<String, Vec<u8>>>,
    pending_signal: Mutex<Option<String>>,
    pending_checkpoint_signal: Mutex<Option<String>>,
    custom_signal_payload: Mutex<Option<Vec<u8>>>,
}

#[derive(Default)]
struct ExecuteOptions {
    preloaded_checkpoints: Vec<(String, Vec<u8>)>,
    pending_signal: Option<String>,
    pending_checkpoint_signal: Option<String>,
    custom_signal_payload: Option<Vec<u8>>,
    debug_mode: bool,
}

impl ServerState {
    fn new(
        preloaded_checkpoints: Vec<(String, Vec<u8>)>,
        pending_signal: Option<String>,
        pending_checkpoint_signal: Option<String>,
        custom_signal_payload: Option<Vec<u8>>,
    ) -> Self {
        Self {
            checkpoints: Mutex::new(preloaded_checkpoints.into_iter().collect()),
            pending_signal: Mutex::new(pending_signal),
            pending_checkpoint_signal: Mutex::new(pending_checkpoint_signal),
            custom_signal_payload: Mutex::new(custom_signal_payload),
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
        let required_stdlib_markers: &[&[u8]] = &[
            b"split-cache-key",
            b"wait-signal-id",
            b"wait-output",
            b"wait-debug-start",
            b"wait-timeout-error",
            b"wait-on-wait-variables",
            b"wait-on-wait-error",
            b"breakpoint-key",
            b"breakpoint-event",
            b"embed-workflow-cache-key",
            b"embed-workflow-variables",
            b"embed-workflow-result",
            b"embed-workflow-output-from-result",
            b"embed-workflow-error",
            b"retry-sleep-key",
            b"retry-delay-ms",
            b"workflow-error-retryable",
            b"workflow-error-rate-limited",
            b"workflow-error-retry-after-ms",
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

fn direct_breakpoint_json(graph_json: &str, step_id: &str) -> String {
    let mut graph: Value = serde_json::from_str(graph_json).expect("fixture parses");
    graph["durable"] = serde_json::json!(true);
    graph["steps"][step_id]["breakpoint"] = serde_json::json!(true);
    serde_json::to_string(&graph).expect("fixture serializes")
}

fn finish_breakpoint_json() -> String {
    direct_breakpoint_json(SIMPLE_PASSTHROUGH, "finish")
}

fn delay_breakpoint_json() -> String {
    direct_breakpoint_json(DELAY_DYNAMIC, "delay")
}

fn split_breakpoint_json() -> String {
    direct_breakpoint_json(SPLIT_FINISH_WITH_SCHEMAS, "split")
}

fn while_breakpoint_json() -> String {
    direct_breakpoint_json(WHILE_DIRECT_INDEX_ONLY, "loop")
}

fn embed_workflow_breakpoint_parent_json() -> String {
    let mut graph: Value = serde_json::from_str(EMBED_WORKFLOW).expect("fixture parses");
    graph["durable"] = serde_json::json!(true);
    graph["steps"]["call_child"]["breakpoint"] = serde_json::json!(true);
    serde_json::to_string(&graph).expect("fixture serializes")
}

/// `EMBED_WORKFLOW` with an (unenforced) `timeout` on the embed step, to prove
/// the field is an inert no-op that does not change execution vs. the plain
/// static-child case.
fn embed_workflow_timeout_parent_json() -> String {
    let mut graph: Value = serde_json::from_str(EMBED_WORKFLOW).expect("fixture parses");
    graph["steps"]["call_child"]["timeout"] = serde_json::json!(5_000);
    serde_json::to_string(&graph).expect("fixture serializes")
}

/// `AGENT_RETURN_INPUT` with an (unenforced) `timeout` on the agent step.
fn agent_timeout_json() -> String {
    let mut graph: Value = serde_json::from_str(AGENT_RETURN_INPUT).expect("fixture parses");
    graph["steps"]["agent"]["timeout"] = serde_json::json!(5_000);
    serde_json::to_string(&graph).expect("fixture serializes")
}

fn embed_workflow_child_workflows() -> Vec<ChildWorkflowInput> {
    embed_workflow_child_workflows_with_graph(EMBED_WORKFLOW_FINISH_CHILD)
}

fn embed_workflow_error_child_workflows() -> Vec<ChildWorkflowInput> {
    embed_workflow_child_workflows_with_graph(EMBED_WORKFLOW_ERROR_CHILD)
}

fn embed_workflow_transient_error_child_workflows() -> Vec<ChildWorkflowInput> {
    embed_workflow_child_workflows_with_graph(EMBED_WORKFLOW_TRANSIENT_ERROR_CHILD)
}

fn embed_workflow_nested_retry_child_workflows() -> Vec<ChildWorkflowInput> {
    vec![
        ChildWorkflowInput {
            step_id: "call_child".to_string(),
            workflow_id: "child_workflow".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 3,
            execution_graph: graph_from_fixture(EMBED_WORKFLOW_RETRY_NESTED_CHILD),
        },
        ChildWorkflowInput {
            step_id: "call_grandchild".to_string(),
            workflow_id: "grandchild_workflow".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 7,
            execution_graph: graph_from_fixture(EMBED_WORKFLOW_TRANSIENT_ERROR_GRANDCHILD),
        },
    ]
}

fn embed_workflow_child_local_on_error_child_workflows() -> Vec<ChildWorkflowInput> {
    vec![
        ChildWorkflowInput {
            step_id: "call_child".to_string(),
            workflow_id: "child_workflow".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 3,
            execution_graph: graph_from_fixture(EMBED_WORKFLOW_CHILD_LOCAL_ON_ERROR_CHILD),
        },
        ChildWorkflowInput {
            step_id: "call_grandchild".to_string(),
            workflow_id: "grandchild_workflow".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 7,
            execution_graph: graph_from_fixture(EMBED_WORKFLOW_TRANSIENT_ERROR_GRANDCHILD),
        },
    ]
}

fn embed_workflow_conditional_error_child_workflows() -> Vec<ChildWorkflowInput> {
    embed_workflow_child_workflows_with_graph(EMBED_WORKFLOW_CONDITIONAL_ERROR_CHILD)
}

fn embed_workflow_nested_child_workflows() -> Vec<ChildWorkflowInput> {
    embed_workflow_nested_child_workflows_with_great_grandchild(
        EMBED_WORKFLOW_NESTED_GREAT_GRANDCHILD,
    )
}

fn embed_workflow_nested_error_child_workflows() -> Vec<ChildWorkflowInput> {
    embed_workflow_nested_child_workflows_with_great_grandchild(
        EMBED_WORKFLOW_NESTED_ERROR_GREAT_GRANDCHILD,
    )
}

fn embed_workflow_nested_child_workflows_with_great_grandchild(
    great_grandchild_graph: &str,
) -> Vec<ChildWorkflowInput> {
    vec![
        ChildWorkflowInput {
            step_id: "call_child".to_string(),
            workflow_id: "child_workflow".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 3,
            execution_graph: graph_from_fixture(EMBED_WORKFLOW_NESTED_CHILD),
        },
        ChildWorkflowInput {
            step_id: "call_grandchild".to_string(),
            workflow_id: "grandchild_workflow".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 7,
            execution_graph: graph_from_fixture(EMBED_WORKFLOW_NESTED_GRANDCHILD),
        },
        ChildWorkflowInput {
            step_id: "call_greatgrandchild".to_string(),
            workflow_id: "great_grandchild_workflow".to_string(),
            version_requested: "latest".to_string(),
            version_resolved: 11,
            execution_graph: graph_from_fixture(great_grandchild_graph),
        },
    ]
}

fn embed_workflow_child_workflows_with_graph(graph_json: &str) -> Vec<ChildWorkflowInput> {
    vec![ChildWorkflowInput {
        step_id: "call_child".to_string(),
        workflow_id: "child_workflow".to_string(),
        version_requested: "latest".to_string(),
        version_resolved: 3,
        execution_graph: graph_from_fixture(graph_json),
    }]
}

fn compile_components_artifact(workflow_id: &str, graph_json: &str) -> PathBuf {
    compile_components_artifact_with_tracking(workflow_id, graph_json, false)
}

fn compile_components_artifact_with_tracking(
    workflow_id: &str,
    graph_json: &str,
    track_events: bool,
) -> PathBuf {
    compile_components_artifact_with_child_workflows_and_tracking(
        workflow_id,
        graph_json,
        track_events,
        &[],
    )
}

fn compile_components_artifact_with_child_workflows(
    workflow_id: &str,
    graph_json: &str,
    child_workflows: &[ChildWorkflowInput],
) -> PathBuf {
    compile_components_artifact_with_child_workflows_and_tracking(
        workflow_id,
        graph_json,
        false,
        child_workflows,
    )
}

fn compile_components_artifact_with_child_workflows_and_tracking(
    workflow_id: &str,
    graph_json: &str,
    track_events: bool,
    child_workflows: &[ChildWorkflowInput],
) -> PathBuf {
    let compiled = compile_workflow(CompilationInput {
        tenant_id: "direct-wasm-ab".to_string(),
        workflow_id: format!("ab-components-{workflow_id}"),
        version: 1,
        execution_graph: graph_from_fixture(graph_json),
        track_events,
        child_workflows: child_workflows.to_vec(),
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
    compile_direct_artifact_with_tracking(components_dir, workflow_id, graph_json, false)
}

fn compile_direct_artifact_with_tracking(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    track_events: bool,
) -> DirectArtifact {
    compile_direct_artifact_with_child_workflows_and_tracking(
        components_dir,
        workflow_id,
        graph_json,
        track_events,
        &[],
    )
}

fn compile_direct_artifact_with_child_workflows(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    child_workflows: &[ChildWorkflowInput],
) -> DirectArtifact {
    compile_direct_artifact_with_child_workflows_and_tracking(
        components_dir,
        workflow_id,
        graph_json,
        false,
        child_workflows,
    )
}

fn compile_direct_artifact_with_child_workflows_and_tracking(
    components_dir: &Path,
    workflow_id: &str,
    graph_json: &str,
    track_events: bool,
    child_workflows: &[ChildWorkflowInput],
) -> DirectArtifact {
    let temp = TempDir::new().expect("tempdir");
    let compiled = compile_workflow_direct(
        CompilationInput {
            tenant_id: "direct-wasm-ab".to_string(),
            workflow_id: format!("ab-direct-{workflow_id}"),
            version: 1,
            execution_graph: graph_from_fixture(graph_json),
            track_events,
            child_workflows: child_workflows.to_vec(),
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

    // Deterministic mock LLM proxy. `runtara-http::call_agent` posts the proxy
    // envelope `{method, url, headers, body, connection_id, ...}` here when
    // `RUNTARA_HTTP_PROXY_URL` points at this server. We return a canned OpenAI
    // chat-completion response so AiAgent runs are deterministic and identical
    // across the generated and direct artifacts (both call through this mock).
    //
    // Turn-aware for the tool loop: when the request advertises tools and the
    // conversation is still short (first turn), return a tool call; otherwise
    // (no tools, or a later turn that already has the tool result) return text.
    if method == "POST" && path == "/proxy" {
        let envelope: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let llm_request = envelope.get("body").cloned().unwrap_or(Value::Null);

        // MCP JSON-RPC envelope: the mcp agent posts `tools/list` / `tools/call`
        // through the same proxy. Respond as a deterministic MCP server.
        if let Some(rpc) = llm_request.get("method").and_then(Value::as_str) {
            let rpc_body = match rpc {
                "tools/list" => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": llm_request.get("id").cloned().unwrap_or(Value::from(1)),
                    "result": {
                        "tools": [{
                            "name": "echo_tool",
                            "description": "Echo back the provided value.",
                            "inputSchema": {
                                "type": "object",
                                "properties": { "value": { "type": "string" } },
                                "required": ["value"]
                            }
                        }]
                    }
                }),
                _ => serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": llm_request.get("id").cloned().unwrap_or(Value::from(1)),
                    "result": {
                        "content": [{ "type": "text", "text": "echo: from-mcp" }],
                        "isError": false
                    }
                }),
            };
            return (
                200,
                serde_json::json!({ "status": 200, "headers": {}, "body": rpc_body }),
            );
        }

        let messages = llm_request
            .get("messages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let message_count = messages.len();
        let tools = llm_request
            .get("tools")
            .and_then(Value::as_array)
            .filter(|tools| !tools.is_empty());

        // MCP meta-tools advertise a `<toolset>_search` / `<toolset>_invoke`
        // pair. Drive the three-step flow search → invoke → text, selecting the
        // step from how many assistant tool calls already happened.
        let mcp_tool = |suffix: &str| -> Option<String> {
            tools?.iter().find_map(|tool| {
                let name = tool
                    .get("function")
                    .and_then(|f| f.get("name"))
                    .or_else(|| tool.get("name"))
                    .and_then(Value::as_str)?;
                name.ends_with(suffix).then(|| name.to_string())
            })
        };
        let mcp_search = mcp_tool("_search");
        let mcp_invoke = mcp_tool("_invoke");
        let prior_tool_calls = messages
            .iter()
            .filter(|m| {
                m.get("role").and_then(Value::as_str) == Some("assistant")
                    && m.get("tool_calls")
                        .and_then(Value::as_array)
                        .is_some_and(|calls| !calls.is_empty())
            })
            .count();

        let tool_call = |name: String, arguments: &str| {
            (
                serde_json::json!({
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": { "name": name, "arguments": arguments }
                    }]
                }),
                "tool_calls",
            )
        };

        let (message, finish_reason) =
            if let (Some(search), Some(invoke)) = (mcp_search.clone(), mcp_invoke.clone()) {
                match prior_tool_calls {
                    0 => tool_call(search, "{\"query\":\"echo\"}"),
                    1 => tool_call(
                        invoke,
                        "{\"tool_name\":\"echo_tool\",\"args\":{\"value\":\"x\"}}",
                    ),
                    _ => (
                        serde_json::json!({ "role": "assistant", "content": MOCK_AI_RESPONSE }),
                        "stop",
                    ),
                }
            } else if tools.is_some() && message_count <= 2 {
                // Regular Agent tool loop: call the last advertised tool, exercising
                // a non-zero tool index in the multi-tool case.
                let tool_name = tools
                    .and_then(|tools| tools.last())
                    .and_then(|tool| tool.get("function").and_then(|f| f.get("name")))
                    .or_else(|| {
                        tools
                            .and_then(|tools| tools.last())
                            .and_then(|tool| tool.get("name"))
                    })
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string();
                tool_call(tool_name, "{\"value\":\"from-tool\"}")
            } else {
                (
                    serde_json::json!({ "role": "assistant", "content": MOCK_AI_RESPONSE }),
                    "stop",
                )
            };

        return (
            200,
            serde_json::json!({
                "status": 200,
                "headers": {},
                "body": {
                    "id": "chatcmpl-mock",
                    "model": "gpt-4o",
                    "choices": [{
                        "index": 0,
                        "message": message,
                        "finish_reason": finish_reason
                    }],
                    "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
                }
            }),
        );
    }

    // Mock object-model API for AiAgent conversation memory. The object-model
    // agent calls these directly (not via the proxy) at
    // `RUNTARA_OBJECT_MODEL_URL`. Loads return an empty conversation; schema and
    // save operations succeed. This is enough for deterministic memory A/B runs.
    if let Some(om_path) = path.strip_prefix("/api/internal/object-model") {
        if method == "GET" && om_path.starts_with("/schemas/") {
            return (
                200,
                serde_json::json!({ "success": true, "schema": { "name": "_ai_conversation_memory" } }),
            );
        }
        if method == "POST" && om_path == "/instances/query" {
            return (200, serde_json::json!({ "success": true, "instances": [] }));
        }
        if method == "POST" && om_path == "/instances" {
            // save-memory create: capture the persisted (post-compaction)
            // conversation messages so the test can assert compaction parity.
            if let Some(messages) = serde_json::from_slice::<Value>(body)
                .ok()
                .and_then(|payload| {
                    payload
                        .get("properties")?
                        .get("messages")?
                        .as_array()
                        .cloned()
                })
            {
                let _ = sink.send(CapturedMessage::MemorySave(messages));
            }
            return (
                200,
                serde_json::json!({ "success": true, "id": "mem-1", "instance": { "id": "mem-1" } }),
            );
        }
        // save-memory update: capture the persisted messages from `data`.
        if method == "PUT" && om_path.starts_with("/instances/") {
            if let Some(messages) = serde_json::from_slice::<Value>(body)
                .ok()
                .and_then(|payload| payload.get("data")?.get("messages")?.as_array().cloned())
            {
                let _ = sink.send(CapturedMessage::MemorySave(messages));
            }
            return (200, serde_json::json!({ "success": true }));
        }
        // /schemas create, etc.
        return (200, serde_json::json!({ "success": true }));
    }

    // Connection-service mock for the MCP agent. `resolve_connection_params`
    // GETs `{CONNECTION_SERVICE_URL}/{tenant}/{connection_id}` when the invoke
    // passed empty parameters; it expects `{parameters: {...}}`. The `url` only
    // needs to be non-empty — the mcp client posts JSON-RPC through the proxy
    // (handled above), not to this url directly.
    if method == "GET" && path.starts_with("/connsvc/") {
        return (
            200,
            serde_json::json!({
                "parameters": { "url": "http://mcp-mock.local/rpc" }
            }),
        );
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
            ("GET", "signals") => {
                let signal = server_state
                    .pending_signal
                    .lock()
                    .expect("signal state lock")
                    .take()
                    .map(|signal_type| {
                        serde_json::json!({
                            "signal_type": signal_type,
                            "payload": null,
                        })
                    });
                return (
                    200,
                    serde_json::json!({
                        "signal": signal,
                        "custom_signal": null,
                    }),
                );
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
            ("POST", "retry") => {
                capture_retry_attempt(body, sink);
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

        if method == "GET"
            && let Some(checkpoint_id) = endpoint.strip_prefix("signals/")
        {
            let custom_signal = server_state
                .custom_signal_payload
                .lock()
                .expect("custom signal lock")
                .take()
                .map(|payload| {
                    serde_json::json!({
                        "checkpoint_id": checkpoint_id,
                        "payload": base64::engine::general_purpose::STANDARD.encode(payload),
                    })
                });
            return (
                200,
                serde_json::json!({
                    "signal": null,
                    "custom_signal": custom_signal,
                }),
            );
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

fn capture_retry_attempt(body: &[u8], sink: &mpsc::Sender<CapturedMessage>) {
    if let Ok(parsed) = serde_json::from_slice::<Value>(body)
        && let Some(checkpoint_id) = parsed.get("checkpoint_id").and_then(Value::as_str)
    {
        let attempt = parsed
            .get("attempt")
            .and_then(Value::as_u64)
            .and_then(|attempt| u32::try_from(attempt).ok())
            .unwrap_or_default();
        let error_json = parsed
            .get("error_message")
            .and_then(Value::as_str)
            .map(|error| {
                serde_json::from_str::<Value>(error)
                    .unwrap_or_else(|_| Value::String(error.to_string()))
            });
        let _ = sink.send(CapturedMessage::RetryAttempt(RetryAttemptRequest {
            checkpoint_id: checkpoint_id.to_string(),
            attempt,
            error_json,
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
    execute_artifact_with_options(
        binary_path,
        instance_id,
        workflow_input,
        ExecuteOptions {
            preloaded_checkpoints,
            ..ExecuteOptions::default()
        },
    )
}

fn execute_artifact_with_checkpoint_signal(
    binary_path: &Path,
    instance_id: &str,
    workflow_input: &[u8],
    signal_type: &str,
) -> CapturedRun {
    execute_artifact_with_options(
        binary_path,
        instance_id,
        workflow_input,
        ExecuteOptions {
            pending_checkpoint_signal: Some(signal_type.to_string()),
            ..ExecuteOptions::default()
        },
    )
}

fn execute_artifact_with_signal(
    binary_path: &Path,
    instance_id: &str,
    workflow_input: &[u8],
    signal_type: &str,
) -> CapturedRun {
    execute_artifact_with_options(
        binary_path,
        instance_id,
        workflow_input,
        ExecuteOptions {
            pending_signal: Some(signal_type.to_string()),
            ..ExecuteOptions::default()
        },
    )
}

fn execute_artifact_with_custom_signal(
    binary_path: &Path,
    instance_id: &str,
    workflow_input: &[u8],
    signal_payload: &[u8],
) -> CapturedRun {
    execute_artifact_with_options(
        binary_path,
        instance_id,
        workflow_input,
        ExecuteOptions {
            custom_signal_payload: Some(signal_payload.to_vec()),
            ..ExecuteOptions::default()
        },
    )
}

fn execute_artifact_with_debug_mode(
    binary_path: &Path,
    instance_id: &str,
    workflow_input: &[u8],
) -> CapturedRun {
    execute_artifact_with_options(
        binary_path,
        instance_id,
        workflow_input,
        ExecuteOptions {
            debug_mode: true,
            ..ExecuteOptions::default()
        },
    )
}

fn execute_artifact_with_checkpoint_and_custom_signal_debug_mode(
    binary_path: &Path,
    instance_id: &str,
    workflow_input: &[u8],
    preloaded_checkpoints: Vec<(String, Vec<u8>)>,
    signal_payload: &[u8],
) -> CapturedRun {
    execute_artifact_with_options(
        binary_path,
        instance_id,
        workflow_input,
        ExecuteOptions {
            preloaded_checkpoints,
            custom_signal_payload: Some(signal_payload.to_vec()),
            debug_mode: true,
            ..ExecuteOptions::default()
        },
    )
}

fn execute_artifact_with_options(
    binary_path: &Path,
    instance_id: &str,
    workflow_input: &[u8],
    options: ExecuteOptions,
) -> CapturedRun {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let (capture_tx, capture_rx) = mpsc::channel::<CapturedMessage>();
    let (stop_tx, stop_rx) = mpsc::channel::<()>();
    let server_state = Arc::new(ServerState::new(
        options.preloaded_checkpoints,
        options.pending_signal,
        options.pending_checkpoint_signal,
        options.custom_signal_payload,
    ));
    let workflow_input = Arc::new(workflow_input.to_vec());
    let server_handle =
        thread::spawn(move || serve(listener, capture_tx, stop_rx, server_state, workflow_input));

    let mut command = Command::new(wasmtime_binary());
    command
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
        // Route agent HTTP egress (e.g. the AiAgent chat-completion LLM call)
        // through the in-test mock proxy served by `route`'s `/proxy` handler.
        .arg("--env")
        .arg(format!("RUNTARA_HTTP_PROXY_URL=http://{addr}/proxy"))
        // The object-model agent (AiAgent memory provider) calls this directly.
        .arg("--env")
        .arg(format!(
            "RUNTARA_OBJECT_MODEL_URL=http://{addr}/api/internal/object-model"
        ))
        // The mcp agent resolves empty connection parameters via the
        // connection-service mock (`{base}/{tenant}/{connection_id}`).
        .arg("--env")
        .arg(format!("CONNECTION_SERVICE_URL=http://{addr}/connsvc"))
        .arg("--env")
        .arg("RUST_LOG=warn");
    if options.debug_mode {
        command.arg("--env").arg("DEBUG_MODE=true");
    }

    let output = command
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
    let mut retry_attempts = Vec::new();
    let mut suspended_count = 0usize;
    let mut signal_acks = Vec::new();
    let mut memory_saves = Vec::new();
    for message in capture_rx.try_iter() {
        match message {
            CapturedMessage::Completed(completed) => output_json = Some(completed.output_json),
            CapturedMessage::Failed(failed) => error_json = Some(failed.error_json),
            CapturedMessage::Event(event) => events.push(event),
            CapturedMessage::Sleep(sleep) => sleeps.push(sleep),
            CapturedMessage::Checkpoint(checkpoint) => checkpoints.push(checkpoint),
            CapturedMessage::RetryAttempt(retry_attempt) => retry_attempts.push(retry_attempt),
            CapturedMessage::Suspended => suspended_count += 1,
            CapturedMessage::SignalAck(signal_ack) => signal_acks.push(signal_ack),
            CapturedMessage::MemorySave(messages) => memory_saves.push(messages),
        }
    }

    CapturedRun {
        output_json,
        error_json,
        events,
        sleeps,
        checkpoints,
        retry_attempts,
        suspended_count,
        signal_acks,
        memory_saves,
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

fn remove_direct_workflow_id_variable(value: &mut Value) {
    if let Some(variables) = value.get_mut("variables").and_then(Value::as_object_mut) {
        variables.remove("_workflow_id");
    }
    if let Some(variables) = value
        .pointer_mut("/workflow/inputs/variables")
        .and_then(Value::as_object_mut)
    {
        variables.remove("_workflow_id");
    }
}

fn normalized_event_payload(subtype: &str, mut payload: Value) -> Value {
    if let Some(object) = payload.as_object_mut() {
        object.remove("timestamp_ms");
        if subtype == "step_debug_end" {
            object.remove("duration_ms");
        }
        if subtype == "external_input_requested" {
            object.remove("signal_id");
        }
        if subtype == "breakpoint_hit"
            && let Some(inputs) = object.get_mut("inputs")
        {
            remove_direct_workflow_id_variable(inputs);
        }
        if object.get("step_type").and_then(Value::as_str) == Some("WaitForSignal") {
            if let Some(inputs) = object.get_mut("inputs").and_then(Value::as_object_mut) {
                inputs.remove("signal_id");
            }
            if let Some(outputs) = object.get_mut("outputs").and_then(Value::as_object_mut) {
                outputs.remove("signal_id");
            }
        }
    }
    payload
}

fn normalized_events(events: &[RuntimeEvent]) -> Vec<(String, Value)> {
    events
        .iter()
        .map(|event| {
            (
                event.subtype.clone(),
                normalized_event_payload(&event.subtype, event.payload_json.clone()),
            )
        })
        .collect()
}

fn normalized_failure_error(error_json: &Option<Value>) -> Option<Value> {
    let Value::String(error) = error_json.as_ref()? else {
        return error_json.clone();
    };
    // Generated wraps a fatal Split item failure as a non-JSON string
    // "Split step failed at index N: {json}"; the direct emitter preserves the
    // structured error object instead of inheriting that lossy string wrapping.
    // Normalize the generated form back to the structured error for comparison.
    if let Some((prefix, payload)) = error.split_once(": ")
        && prefix.starts_with("Split step failed at index ")
        && let Ok(parsed) = serde_json::from_str::<Value>(payload)
    {
        return Some(parsed);
    }
    let Some(prefix) = error
        .split_once(" waiting for signal '")
        .map(|(prefix, _)| prefix)
    else {
        return error_json.clone();
    };
    if !prefix.starts_with("WaitForSignal step '") || !prefix.contains("' timed out after ") {
        return error_json.clone();
    }
    Some(Value::String(format!(
        "{prefix} waiting for signal '<signal>'"
    )))
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
        .or_else(|| checkpoint_id.find("embed_workflow::"))
        .or_else(|| checkpoint_id.find("breakpoint::"))
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

fn normalized_retry_attempts(
    retry_attempts: &[RetryAttemptRequest],
) -> Vec<(String, u32, Option<Value>)> {
    retry_attempts
        .iter()
        .map(|retry| {
            (
                normalized_checkpoint_id(&retry.checkpoint_id),
                retry.attempt,
                normalized_failure_error(&retry.error_json),
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
        normalized_failure_error(&components.error_json),
        normalized_failure_error(&direct.error_json),
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
fn direct_wasm_matches_components_finish_breakpoint_pause_resume() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let graph_json = finish_breakpoint_json();
    let components_artifact = compile_components_artifact("finish-breakpoint", &graph_json);
    let direct_artifact =
        compile_direct_artifact(&components_dir, "finish-breakpoint", &graph_json);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"input":"fresh-finish"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components_paused = execute_artifact_with_debug_mode(
        &components_artifact,
        "ab-components-finish-breakpoint-pause",
        &components_input,
    );
    let direct_paused = execute_artifact_with_debug_mode(
        &direct_artifact.path,
        "ab-direct-finish-breakpoint-pause",
        workflow_input,
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
    assert!(components_paused.output_json.is_none());
    assert!(direct_paused.output_json.is_none());
    assert!(components_paused.error_json.is_none());
    assert!(direct_paused.error_json.is_none());

    let expected_checkpoint = vec![(
        "breakpoint::finish".to_string(),
        br#""breakpoint_hit""#.to_vec(),
    )];
    assert_eq!(
        normalized_checkpoints(&components_paused.checkpoints),
        expected_checkpoint
    );
    assert_eq!(
        normalized_checkpoints(&direct_paused.checkpoints),
        expected_checkpoint
    );
    assert_eq!(
        normalized_events(&components_paused.events),
        normalized_events(&direct_paused.events)
    );
    let direct_pause_events = normalized_events(&direct_paused.events);
    let breakpoint_events = direct_pause_events
        .iter()
        .filter(|(subtype, _)| subtype == "breakpoint_hit")
        .collect::<Vec<_>>();
    assert_eq!(breakpoint_events.len(), 1);
    assert_eq!(breakpoint_events[0].1["step_type"], "Finish");
    assert_eq!(
        breakpoint_events[0].1["inputs"],
        serde_json::json!({ "result": "fresh-finish" })
    );
    assert_eq!(components_paused.suspended_count, 1);
    assert_eq!(direct_paused.suspended_count, 1);
    let expected_pause_ack = vec![SignalAckRequest {
        signal_type: "pause".to_string(),
    }];
    assert_eq!(components_paused.signal_acks, expected_pause_ack);
    assert_eq!(direct_paused.signal_acks, expected_pause_ack);

    let components_resumed = execute_artifact_with_options(
        &components_artifact,
        "ab-components-finish-breakpoint-resume",
        &components_input,
        ExecuteOptions {
            preloaded_checkpoints: expected_checkpoint.clone(),
            debug_mode: true,
            ..ExecuteOptions::default()
        },
    );
    let direct_resumed = execute_artifact_with_options(
        &direct_artifact.path,
        "ab-direct-finish-breakpoint-resume",
        workflow_input,
        ExecuteOptions {
            preloaded_checkpoints: expected_checkpoint,
            debug_mode: true,
            ..ExecuteOptions::default()
        },
    );

    assert_success_parity(
        "finish-breakpoint-resume",
        0,
        &components_resumed,
        &direct_resumed,
    );
    assert_eq!(
        direct_resumed.output_json,
        Some(serde_json::json!({ "result": "fresh-finish" }))
    );
    assert!(
        normalized_events(&direct_resumed.events)
            .iter()
            .all(|(subtype, _)| subtype != "breakpoint_hit"),
        "resume from breakpoint checkpoint should not emit a second breakpoint event"
    );
    assert_eq!(components_resumed.suspended_count, 0);
    assert_eq!(direct_resumed.suspended_count, 0);
    assert!(components_resumed.signal_acks.is_empty());
    assert!(direct_resumed.signal_acks.is_empty());
}

#[test]
fn direct_wasm_matches_components_direct_control_breakpoint_pause_resume() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    struct BreakpointCase {
        name: &'static str,
        graph_json: &'static str,
        step_id: &'static str,
        step_type: &'static str,
        workflow_input: &'static [u8],
        input_pointer: &'static str,
        input_value: Value,
        resumes_with_failure: bool,
    }

    let cases = vec![
        BreakpointCase {
            name: "conditional-breakpoint",
            graph_json: CONDITIONAL_WORKFLOW,
            step_id: "check",
            step_type: "Conditional",
            workflow_input: br#"{"flag":true}"#,
            input_pointer: "/data/flag",
            input_value: serde_json::json!(true),
            resumes_with_failure: false,
        },
        BreakpointCase {
            name: "filter-breakpoint",
            graph_json: FILTER_SIMPLE,
            step_id: "filter",
            step_type: "Filter",
            workflow_input: br#"{"items":[{"status":"active"},{"status":"archived"}]}"#,
            input_pointer: "/0/status",
            input_value: serde_json::json!("active"),
            resumes_with_failure: false,
        },
        BreakpointCase {
            name: "switch-value-breakpoint",
            graph_json: SWITCH_VALUE_SIMPLE,
            step_id: "switch",
            step_type: "Switch",
            workflow_input: br#"{"status":"active"}"#,
            input_pointer: "/value",
            input_value: serde_json::json!("active"),
            resumes_with_failure: false,
        },
        BreakpointCase {
            name: "switch-routing-breakpoint",
            graph_json: SWITCH_ROUTING_SIMPLE,
            step_id: "switch",
            step_type: "Switch",
            workflow_input: br#"{"status":"active"}"#,
            input_pointer: "/value",
            input_value: serde_json::json!("active"),
            resumes_with_failure: false,
        },
        BreakpointCase {
            name: "group-by-breakpoint",
            graph_json: GROUP_BY_SIMPLE,
            step_id: "group",
            step_type: "GroupBy",
            workflow_input: br#"{"items":[{"status":"active"},{"status":"archived"}]}"#,
            input_pointer: "/0/status",
            input_value: serde_json::json!("active"),
            resumes_with_failure: false,
        },
        BreakpointCase {
            name: "log-breakpoint",
            graph_json: LOG_ALL_LEVELS,
            step_id: "log_debug",
            step_type: "Log",
            workflow_input: br#"{"message":"hello"}"#,
            input_pointer: "/debugData/message",
            input_value: serde_json::json!("hello"),
            resumes_with_failure: false,
        },
        BreakpointCase {
            name: "error-breakpoint",
            graph_json: ERROR_DIRECT_SIMPLE,
            step_id: "fail",
            step_type: "Error",
            workflow_input: br#"{"requestId":"r-1"}"#,
            input_pointer: "/requestId",
            input_value: serde_json::json!("r-1"),
            resumes_with_failure: true,
        },
    ];

    for case in cases {
        let graph_json = direct_breakpoint_json(case.graph_json, case.step_id);
        let components_artifact = compile_components_artifact(case.name, &graph_json);
        let direct_artifact = compile_direct_artifact(&components_dir, case.name, &graph_json);
        assert_eq!(
            direct_artifact.compiler_mode,
            WorkflowCompilerMode::DirectWasm
        );

        let components_input = components_sdk_input(case.workflow_input);
        let components_paused = execute_artifact_with_debug_mode(
            &components_artifact,
            &format!("ab-components-{}-pause", case.name),
            &components_input,
        );
        let direct_paused = execute_artifact_with_debug_mode(
            &direct_artifact.path,
            &format!("ab-direct-{}-pause", case.name),
            case.workflow_input,
        );

        assert!(
            components_paused.status_success,
            "components artifact did not suspend cleanly for {}:\n{}",
            case.name, components_paused.stderr
        );
        assert!(
            direct_paused.status_success,
            "direct artifact did not suspend cleanly for {}:\n{}",
            case.name, direct_paused.stderr
        );
        assert!(components_paused.output_json.is_none());
        assert!(direct_paused.output_json.is_none());
        assert!(components_paused.error_json.is_none());
        assert!(direct_paused.error_json.is_none());

        let expected_checkpoint = vec![(
            format!("breakpoint::{}", case.step_id),
            br#""breakpoint_hit""#.to_vec(),
        )];
        assert_eq!(
            normalized_checkpoints(&components_paused.checkpoints),
            expected_checkpoint
        );
        assert_eq!(
            normalized_checkpoints(&direct_paused.checkpoints),
            expected_checkpoint
        );
        assert_eq!(
            normalized_events(&components_paused.events),
            normalized_events(&direct_paused.events)
        );
        let direct_pause_events = normalized_events(&direct_paused.events);
        let breakpoint_events = direct_pause_events
            .iter()
            .filter(|(subtype, _)| subtype == "breakpoint_hit")
            .collect::<Vec<_>>();
        assert_eq!(breakpoint_events.len(), 1);
        assert_eq!(breakpoint_events[0].1["step_type"], case.step_type);
        assert_eq!(
            breakpoint_events[0].1["inputs"].pointer(case.input_pointer),
            Some(&case.input_value),
            "{} breakpoint inputs should match generated code",
            case.name
        );
        assert_eq!(components_paused.suspended_count, 1);
        assert_eq!(direct_paused.suspended_count, 1);
        let expected_pause_ack = vec![SignalAckRequest {
            signal_type: "pause".to_string(),
        }];
        assert_eq!(components_paused.signal_acks, expected_pause_ack);
        assert_eq!(direct_paused.signal_acks, expected_pause_ack);

        let components_resumed = execute_artifact_with_options(
            &components_artifact,
            &format!("ab-components-{}-resume", case.name),
            &components_input,
            ExecuteOptions {
                preloaded_checkpoints: expected_checkpoint.clone(),
                debug_mode: true,
                ..ExecuteOptions::default()
            },
        );
        let direct_resumed = execute_artifact_with_options(
            &direct_artifact.path,
            &format!("ab-direct-{}-resume", case.name),
            case.workflow_input,
            ExecuteOptions {
                preloaded_checkpoints: expected_checkpoint,
                debug_mode: true,
                ..ExecuteOptions::default()
            },
        );

        if case.resumes_with_failure {
            assert_failure_parity(case.name, 0, &components_resumed, &direct_resumed);
        } else {
            assert_success_parity(case.name, 0, &components_resumed, &direct_resumed);
        }
        assert!(
            normalized_events(&direct_resumed.events)
                .iter()
                .all(|(subtype, _)| subtype != "breakpoint_hit"),
            "resume from breakpoint checkpoint should not emit a second breakpoint event for {}",
            case.name
        );
        assert_eq!(components_resumed.suspended_count, 0);
        assert_eq!(direct_resumed.suspended_count, 0);
        assert!(components_resumed.signal_acks.is_empty());
        assert!(direct_resumed.signal_acks.is_empty());
    }
}

#[test]
fn direct_wasm_matches_components_agent_breakpoint_pause_resume() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let graph_json = direct_breakpoint_json(AGENT_RETURN_INPUT, "agent");
    let components_artifact = compile_components_artifact("agent-breakpoint", &graph_json);
    let direct_artifact = compile_direct_artifact(&components_dir, "agent-breakpoint", &graph_json);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"value":"fresh-agent"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components_paused = execute_artifact_with_debug_mode(
        &components_artifact,
        "ab-components-agent-breakpoint-pause",
        &components_input,
    );
    let direct_paused = execute_artifact_with_debug_mode(
        &direct_artifact.path,
        "ab-direct-agent-breakpoint-pause",
        workflow_input,
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
    assert!(components_paused.output_json.is_none());
    assert!(direct_paused.output_json.is_none());
    assert!(components_paused.error_json.is_none());
    assert!(direct_paused.error_json.is_none());

    let expected_checkpoint = vec![(
        "breakpoint::agent".to_string(),
        br#""breakpoint_hit""#.to_vec(),
    )];
    assert_eq!(
        normalized_checkpoints(&components_paused.checkpoints),
        expected_checkpoint
    );
    assert_eq!(
        normalized_checkpoints(&direct_paused.checkpoints),
        expected_checkpoint
    );
    assert_eq!(
        normalized_events(&components_paused.events),
        normalized_events(&direct_paused.events)
    );
    let direct_pause_events = normalized_events(&direct_paused.events);
    let breakpoint_events = direct_pause_events
        .iter()
        .filter(|(subtype, _)| subtype == "breakpoint_hit")
        .collect::<Vec<_>>();
    assert_eq!(breakpoint_events.len(), 1);
    assert_eq!(breakpoint_events[0].1["step_type"], "Agent");
    assert_eq!(
        breakpoint_events[0].1["inputs"],
        serde_json::json!({ "value": "fresh-agent" })
    );
    assert_eq!(components_paused.suspended_count, 1);
    assert_eq!(direct_paused.suspended_count, 1);
    let expected_pause_ack = vec![SignalAckRequest {
        signal_type: "pause".to_string(),
    }];
    assert_eq!(components_paused.signal_acks, expected_pause_ack);
    assert_eq!(direct_paused.signal_acks, expected_pause_ack);

    let components_resumed = execute_artifact_with_options(
        &components_artifact,
        "ab-components-agent-breakpoint-resume",
        &components_input,
        ExecuteOptions {
            preloaded_checkpoints: expected_checkpoint.clone(),
            debug_mode: true,
            ..ExecuteOptions::default()
        },
    );
    let direct_resumed = execute_artifact_with_options(
        &direct_artifact.path,
        "ab-direct-agent-breakpoint-resume",
        workflow_input,
        ExecuteOptions {
            preloaded_checkpoints: expected_checkpoint,
            debug_mode: true,
            ..ExecuteOptions::default()
        },
    );

    assert_success_parity(
        "agent-breakpoint-resume",
        0,
        &components_resumed,
        &direct_resumed,
    );

    let expected_output = serde_json::json!({ "result": "fresh-agent" });
    assert_eq!(
        components_resumed.output_json.as_ref(),
        Some(&expected_output)
    );
    assert_eq!(direct_resumed.output_json.as_ref(), Some(&expected_output));
    assert!(
        normalized_events(&direct_resumed.events)
            .iter()
            .all(|(subtype, _)| subtype != "breakpoint_hit"),
        "resume from Agent breakpoint checkpoint should not emit a second breakpoint event"
    );
}

#[test]
fn direct_wasm_matches_components_delay_breakpoint_pause_resume() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let graph_json = delay_breakpoint_json();
    let components_artifact = compile_components_artifact("delay-breakpoint", &graph_json);
    let direct_artifact = compile_direct_artifact(&components_dir, "delay-breakpoint", &graph_json);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"waitTime":0}"#;
    let components_input = components_sdk_input(workflow_input);
    let components_paused = execute_artifact_with_debug_mode(
        &components_artifact,
        "ab-components-delay-breakpoint-pause",
        &components_input,
    );
    let direct_paused = execute_artifact_with_debug_mode(
        &direct_artifact.path,
        "ab-direct-delay-breakpoint-pause",
        workflow_input,
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
    assert!(components_paused.output_json.is_none());
    assert!(direct_paused.output_json.is_none());
    assert!(components_paused.error_json.is_none());
    assert!(direct_paused.error_json.is_none());
    assert!(components_paused.sleeps.is_empty());
    assert!(direct_paused.sleeps.is_empty());

    let expected_checkpoint = vec![(
        "breakpoint::delay".to_string(),
        br#""breakpoint_hit""#.to_vec(),
    )];
    assert_eq!(
        normalized_checkpoints(&components_paused.checkpoints),
        expected_checkpoint
    );
    assert_eq!(
        normalized_checkpoints(&direct_paused.checkpoints),
        expected_checkpoint
    );
    assert_eq!(
        normalized_events(&components_paused.events),
        normalized_events(&direct_paused.events)
    );
    let direct_pause_events = normalized_events(&direct_paused.events);
    let breakpoint_events = direct_pause_events
        .iter()
        .filter(|(subtype, _)| subtype == "breakpoint_hit")
        .collect::<Vec<_>>();
    assert_eq!(breakpoint_events.len(), 1);
    assert_eq!(breakpoint_events[0].1["step_type"], "Delay");
    assert!(breakpoint_events[0].1["inputs"].is_null());
    assert_eq!(components_paused.suspended_count, 1);
    assert_eq!(direct_paused.suspended_count, 1);
    let expected_pause_ack = vec![SignalAckRequest {
        signal_type: "pause".to_string(),
    }];
    assert_eq!(components_paused.signal_acks, expected_pause_ack);
    assert_eq!(direct_paused.signal_acks, expected_pause_ack);

    let components_resumed = execute_artifact_with_options(
        &components_artifact,
        "ab-components-delay-breakpoint-resume",
        &components_input,
        ExecuteOptions {
            preloaded_checkpoints: expected_checkpoint.clone(),
            debug_mode: true,
            ..ExecuteOptions::default()
        },
    );
    let direct_resumed = execute_artifact_with_options(
        &direct_artifact.path,
        "ab-direct-delay-breakpoint-resume",
        workflow_input,
        ExecuteOptions {
            preloaded_checkpoints: expected_checkpoint,
            debug_mode: true,
            ..ExecuteOptions::default()
        },
    );

    assert_success_parity(
        "delay-breakpoint-resume",
        0,
        &components_resumed,
        &direct_resumed,
    );
    assert_eq!(
        direct_resumed.output_json,
        Some(serde_json::json!({ "waited": 0 }))
    );
    assert!(
        normalized_events(&direct_resumed.events)
            .iter()
            .all(|(subtype, _)| subtype != "breakpoint_hit"),
        "resume from breakpoint checkpoint should not emit a second breakpoint event"
    );
    assert_eq!(components_resumed.suspended_count, 0);
    assert_eq!(direct_resumed.suspended_count, 0);
    assert!(components_resumed.signal_acks.is_empty());
    assert!(direct_resumed.signal_acks.is_empty());
}

#[test]
fn direct_wasm_matches_components_split_breakpoint_pause_resume() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let graph_json = split_breakpoint_json();
    let components_artifact = compile_components_artifact("split-breakpoint", &graph_json);
    let direct_artifact = compile_direct_artifact(&components_dir, "split-breakpoint", &graph_json);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"items":[{"value":"split-bp"}]}"#;
    let components_input = components_sdk_input(workflow_input);
    let components_paused = execute_artifact_with_debug_mode(
        &components_artifact,
        "ab-components-split-breakpoint-pause",
        &components_input,
    );
    let direct_paused = execute_artifact_with_debug_mode(
        &direct_artifact.path,
        "ab-direct-split-breakpoint-pause",
        workflow_input,
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
    assert!(components_paused.output_json.is_none());
    assert!(direct_paused.output_json.is_none());
    assert!(components_paused.error_json.is_none());
    assert!(direct_paused.error_json.is_none());

    let expected_checkpoint = vec![(
        "breakpoint::split".to_string(),
        br#""breakpoint_hit""#.to_vec(),
    )];
    assert_eq!(
        normalized_checkpoints(&components_paused.checkpoints),
        expected_checkpoint
    );
    assert_eq!(
        normalized_checkpoints(&direct_paused.checkpoints),
        expected_checkpoint
    );
    assert_eq!(
        normalized_events(&components_paused.events),
        normalized_events(&direct_paused.events)
    );
    let direct_pause_events = normalized_events(&direct_paused.events);
    let breakpoint_events = direct_pause_events
        .iter()
        .filter(|(subtype, _)| subtype == "breakpoint_hit")
        .collect::<Vec<_>>();
    assert_eq!(breakpoint_events.len(), 1);
    assert_eq!(breakpoint_events[0].1["step_type"], "Split");
    assert_eq!(
        breakpoint_events[0].1["inputs"],
        serde_json::json!({
            "value": [{ "value": "split-bp" }],
            "parallelism": 0,
            "sequential": true,
            "dontStopOnFailed": false,
            "allowNull": false,
            "convertSingleValue": false,
            "batchSize": 0
        })
    );
    assert_eq!(components_paused.suspended_count, 1);
    assert_eq!(direct_paused.suspended_count, 1);
    let expected_pause_ack = vec![SignalAckRequest {
        signal_type: "pause".to_string(),
    }];
    assert_eq!(components_paused.signal_acks, expected_pause_ack);
    assert_eq!(direct_paused.signal_acks, expected_pause_ack);

    let components_resumed = execute_artifact_with_options(
        &components_artifact,
        "ab-components-split-breakpoint-resume",
        &components_input,
        ExecuteOptions {
            preloaded_checkpoints: expected_checkpoint.clone(),
            debug_mode: true,
            ..ExecuteOptions::default()
        },
    );
    let direct_resumed = execute_artifact_with_options(
        &direct_artifact.path,
        "ab-direct-split-breakpoint-resume",
        workflow_input,
        ExecuteOptions {
            preloaded_checkpoints: expected_checkpoint,
            debug_mode: true,
            ..ExecuteOptions::default()
        },
    );

    assert_success_parity(
        "split-breakpoint-resume",
        0,
        &components_resumed,
        &direct_resumed,
    );
    assert_eq!(
        direct_resumed.output_json,
        Some(serde_json::json!({
            "results": [{ "value": "split-bp", "index": 0, "indices": [0] }]
        }))
    );
    assert!(
        normalized_events(&direct_resumed.events)
            .iter()
            .all(|(subtype, _)| subtype != "breakpoint_hit"),
        "resume from Split breakpoint checkpoint should not emit a second breakpoint event"
    );
    assert_eq!(components_resumed.suspended_count, 0);
    assert_eq!(direct_resumed.suspended_count, 0);
    assert!(components_resumed.signal_acks.is_empty());
    assert!(direct_resumed.signal_acks.is_empty());
}

#[test]
fn direct_wasm_matches_components_while_breakpoint_pause_resume() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let graph_json = while_breakpoint_json();
    let components_artifact = compile_components_artifact("while-breakpoint", &graph_json);
    let direct_artifact = compile_direct_artifact(&components_dir, "while-breakpoint", &graph_json);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"count":2}"#;
    let components_input = components_sdk_input(workflow_input);
    let components_paused = execute_artifact_with_debug_mode(
        &components_artifact,
        "ab-components-while-breakpoint-pause",
        &components_input,
    );
    let direct_paused = execute_artifact_with_debug_mode(
        &direct_artifact.path,
        "ab-direct-while-breakpoint-pause",
        workflow_input,
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
    assert!(components_paused.output_json.is_none());
    assert!(direct_paused.output_json.is_none());
    assert!(components_paused.error_json.is_none());
    assert!(direct_paused.error_json.is_none());
    assert!(
        components_paused
            .events
            .iter()
            .all(|event| event.subtype != "step_debug_start")
    );
    assert!(
        direct_paused
            .events
            .iter()
            .all(|event| event.subtype != "step_debug_start")
    );

    let expected_checkpoint = vec![(
        "breakpoint::loop".to_string(),
        br#""breakpoint_hit""#.to_vec(),
    )];
    assert_eq!(
        normalized_checkpoints(&components_paused.checkpoints),
        expected_checkpoint
    );
    assert_eq!(
        normalized_checkpoints(&direct_paused.checkpoints),
        expected_checkpoint
    );
    assert_eq!(
        normalized_events(&components_paused.events),
        normalized_events(&direct_paused.events)
    );
    let direct_pause_events = normalized_events(&direct_paused.events);
    let breakpoint_events = direct_pause_events
        .iter()
        .filter(|(subtype, _)| subtype == "breakpoint_hit")
        .collect::<Vec<_>>();
    assert_eq!(breakpoint_events.len(), 1);
    assert_eq!(breakpoint_events[0].1["step_type"], "While");
    assert_eq!(
        breakpoint_events[0].1["inputs"],
        serde_json::json!({ "maxIterations": 5 })
    );
    assert_eq!(components_paused.suspended_count, 1);
    assert_eq!(direct_paused.suspended_count, 1);
    let expected_pause_ack = vec![SignalAckRequest {
        signal_type: "pause".to_string(),
    }];
    assert_eq!(components_paused.signal_acks, expected_pause_ack);
    assert_eq!(direct_paused.signal_acks, expected_pause_ack);

    let components_resumed = execute_artifact_with_options(
        &components_artifact,
        "ab-components-while-breakpoint-resume",
        &components_input,
        ExecuteOptions {
            preloaded_checkpoints: expected_checkpoint.clone(),
            debug_mode: true,
            ..ExecuteOptions::default()
        },
    );
    let direct_resumed = execute_artifact_with_options(
        &direct_artifact.path,
        "ab-direct-while-breakpoint-resume",
        workflow_input,
        ExecuteOptions {
            preloaded_checkpoints: expected_checkpoint,
            debug_mode: true,
            ..ExecuteOptions::default()
        },
    );

    assert_success_parity(
        "while-breakpoint-resume",
        0,
        &components_resumed,
        &direct_resumed,
    );
    assert_eq!(
        direct_resumed.output_json,
        Some(serde_json::json!({
            "iterations": 2,
            "last": {
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
        }))
    );
    assert!(
        normalized_events(&direct_resumed.events)
            .iter()
            .all(|(subtype, _)| subtype != "breakpoint_hit"),
        "resume from While breakpoint checkpoint should not emit a second breakpoint event"
    );
    assert_eq!(components_resumed.suspended_count, 0);
    assert_eq!(direct_resumed.suspended_count, 0);
    assert!(components_resumed.signal_acks.is_empty());
    assert!(direct_resumed.signal_acks.is_empty());
}

#[test]
fn direct_wasm_matches_components_wait_for_signal_resume() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact =
        compile_components_artifact("wait-signal", WAIT_FOR_SIGNAL_DIRECT_SIMPLE);
    let direct_artifact = compile_direct_artifact(
        &components_dir,
        "wait-signal",
        WAIT_FOR_SIGNAL_DIRECT_SIMPLE,
    );
    let workflow_input = br#"{"case_id":"case-42","summary":"Needs approval"}"#;
    let signal_payload = br#"{"approved":true}"#;
    let components_input = components_sdk_input(workflow_input);

    let components = execute_artifact_with_custom_signal(
        &components_artifact,
        "ab-components-wait-signal-0",
        &components_input,
        signal_payload,
    );
    let direct = execute_artifact_with_custom_signal(
        &direct_artifact.path,
        "ab-direct-wait-signal-0",
        workflow_input,
        signal_payload,
    );

    assert_success_parity("wait-signal", 0, &components, &direct);
    assert_eq!(
        direct.output_json,
        Some(serde_json::json!({"approved": true}))
    );
    let direct_events = normalized_events(&direct.events);
    let (_, wait_event_payload) = direct_events
        .iter()
        .find(|(subtype, _)| subtype == "external_input_requested")
        .expect("direct WaitForSignal run should emit an external input request event");
    assert_eq!(
        wait_event_payload["type"],
        serde_json::json!("external_input_requested")
    );
    assert_eq!(wait_event_payload["step_id"], serde_json::json!("wait"));
    assert_eq!(
        wait_event_payload["step_name"],
        serde_json::json!("Approval")
    );
    assert_eq!(
        wait_event_payload["action_key"],
        serde_json::json!("approval_decision")
    );
    assert_eq!(
        wait_event_payload["correlation"],
        serde_json::json!({ "case_id": "case-42" })
    );
    assert_eq!(
        wait_event_payload["context"],
        serde_json::json!({ "summary": "Needs approval" })
    );
    assert_eq!(
        wait_event_payload["response_schema"],
        serde_json::json!({
            "approved": {
                "type": "boolean",
                "required": true
            }
        })
    );
}

#[test]
fn direct_wasm_matches_components_wait_for_signal_track_events_resume() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact = compile_components_artifact_with_tracking(
        "wait-signal-track-events",
        WAIT_FOR_SIGNAL_DIRECT_SIMPLE,
        true,
    );
    let direct_artifact = compile_direct_artifact_with_tracking(
        &components_dir,
        "wait-signal-track-events",
        WAIT_FOR_SIGNAL_DIRECT_SIMPLE,
        true,
    );
    let workflow_input = br#"{"case_id":"case-42","summary":"Needs approval"}"#;
    let signal_payload = br#"{"approved":true}"#;
    let components_input = components_sdk_input(workflow_input);

    let components = execute_artifact_with_custom_signal(
        &components_artifact,
        "ab-components-wait-signal-track-events-0",
        &components_input,
        signal_payload,
    );
    let direct = execute_artifact_with_custom_signal(
        &direct_artifact.path,
        "ab-direct-wait-signal-track-events-0",
        workflow_input,
        signal_payload,
    );

    assert_success_parity("wait-signal-track-events", 0, &components, &direct);
    let direct_events = normalized_events(&direct.events);
    assert!(
        direct_events.iter().any(|(subtype, payload)| {
            subtype == "step_debug_start"
                && payload["step_type"] == serde_json::json!("WaitForSignal")
                && payload["inputs"]["poll_interval_ms"] == serde_json::json!(0)
                && payload["inputs"]["response_schema"]
                    == serde_json::json!({
                        "approved": {
                            "type": "boolean",
                            "required": true
                        }
                    })
        }),
        "tracked direct WaitForSignal run should emit a wait debug-start event"
    );
    assert!(
        direct_events.iter().any(|(subtype, payload)| {
            subtype == "step_debug_end"
                && payload["step_type"] == serde_json::json!("WaitForSignal")
                && payload["outputs"]["outputs"] == serde_json::json!({ "approved": true })
        }),
        "tracked direct WaitForSignal run should emit a wait debug-end event"
    );
}

#[test]
fn direct_wasm_matches_components_wait_for_signal_breakpoint_pause_resume() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact =
        compile_components_artifact("wait-signal-breakpoint", WAIT_FOR_SIGNAL_DIRECT_BREAKPOINT);
    let direct_artifact = compile_direct_artifact(
        &components_dir,
        "wait-signal-breakpoint",
        WAIT_FOR_SIGNAL_DIRECT_BREAKPOINT,
    );
    let workflow_input = br#"{"case_id":"case-42","summary":"Needs approval"}"#;
    let components_input = components_sdk_input(workflow_input);

    let components_paused = execute_artifact_with_debug_mode(
        &components_artifact,
        "ab-components-wait-signal-breakpoint-pause",
        &components_input,
    );
    let direct_paused = execute_artifact_with_debug_mode(
        &direct_artifact.path,
        "ab-direct-wait-signal-breakpoint-pause",
        workflow_input,
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
    assert!(components_paused.output_json.is_none());
    assert!(direct_paused.output_json.is_none());
    assert!(components_paused.error_json.is_none());
    assert!(direct_paused.error_json.is_none());

    let expected_checkpoint = vec![(
        "breakpoint::wait".to_string(),
        br#""breakpoint_hit""#.to_vec(),
    )];
    assert_eq!(
        normalized_checkpoints(&components_paused.checkpoints),
        expected_checkpoint
    );
    assert_eq!(
        normalized_checkpoints(&direct_paused.checkpoints),
        expected_checkpoint
    );
    assert_eq!(
        normalized_events(&components_paused.events),
        normalized_events(&direct_paused.events)
    );
    let direct_pause_events = normalized_events(&direct_paused.events);
    let breakpoint_events = direct_pause_events
        .iter()
        .filter(|(subtype, _)| subtype == "breakpoint_hit")
        .collect::<Vec<_>>();
    assert_eq!(breakpoint_events.len(), 1);
    assert_eq!(breakpoint_events[0].1["step_type"], "WaitForSignal");
    assert_eq!(breakpoint_events[0].1["step_name"], "Approval");
    assert!(breakpoint_events[0].1["inputs"].is_null());
    assert!(
        direct_pause_events
            .iter()
            .all(|(subtype, _)| subtype != "external_input_requested"),
        "first breakpoint hit should pause before WaitForSignal request emission"
    );
    assert_eq!(components_paused.suspended_count, 1);
    assert_eq!(direct_paused.suspended_count, 1);
    let expected_pause_ack = vec![SignalAckRequest {
        signal_type: "pause".to_string(),
    }];
    assert_eq!(components_paused.signal_acks, expected_pause_ack);
    assert_eq!(direct_paused.signal_acks, expected_pause_ack);

    let signal_payload = br#"{"approved":true}"#;
    let components_resumed = execute_artifact_with_checkpoint_and_custom_signal_debug_mode(
        &components_artifact,
        "ab-components-wait-signal-breakpoint-resume",
        &components_input,
        expected_checkpoint.clone(),
        signal_payload,
    );
    let direct_resumed = execute_artifact_with_checkpoint_and_custom_signal_debug_mode(
        &direct_artifact.path,
        "ab-direct-wait-signal-breakpoint-resume",
        workflow_input,
        expected_checkpoint,
        signal_payload,
    );

    assert_success_parity(
        "wait-signal-breakpoint-resume",
        0,
        &components_resumed,
        &direct_resumed,
    );
    assert_eq!(
        direct_resumed.output_json,
        Some(serde_json::json!({"approved": true}))
    );
    assert!(
        normalized_events(&direct_resumed.events)
            .iter()
            .all(|(subtype, _)| subtype != "breakpoint_hit"),
        "resume from breakpoint checkpoint should not emit a second breakpoint event"
    );
    assert_eq!(components_resumed.suspended_count, 0);
    assert_eq!(direct_resumed.suspended_count, 0);
    assert!(components_resumed.signal_acks.is_empty());
    assert!(direct_resumed.signal_acks.is_empty());
}

#[test]
fn direct_wasm_matches_components_wait_for_signal_on_wait_callback() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact =
        compile_components_artifact("wait-signal-on-wait", WAIT_FOR_SIGNAL_DIRECT_ON_WAIT);
    let direct_artifact = compile_direct_artifact(
        &components_dir,
        "wait-signal-on-wait",
        WAIT_FOR_SIGNAL_DIRECT_ON_WAIT,
    );
    let workflow_input = br#"{"case_id":"case-onwait","summary":"Notify before wait"}"#;
    let signal_payload = br#"{"approved":true}"#;
    let components_input = components_sdk_input(workflow_input);

    let components = execute_artifact_with_custom_signal(
        &components_artifact,
        "ab-components-wait-signal-on-wait-0",
        &components_input,
        signal_payload,
    );
    let direct = execute_artifact_with_custom_signal(
        &direct_artifact.path,
        "ab-direct-wait-signal-on-wait-0",
        workflow_input,
        signal_payload,
    );

    assert_success_parity("wait-signal-on-wait", 0, &components, &direct);
    assert_eq!(
        direct.output_json,
        Some(serde_json::json!({"approved": true}))
    );
    assert_eq!(
        normalized_events(&direct.events)
            .iter()
            .map(|(subtype, _)| subtype.as_str())
            .collect::<Vec<_>>(),
        vec!["workflow_log", "external_input_requested"]
    );
}

#[test]
fn direct_wasm_matches_components_wait_for_signal_on_wait_error() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact = compile_components_artifact(
        "wait-signal-on-wait-error",
        WAIT_FOR_SIGNAL_DIRECT_ON_WAIT_ERROR,
    );
    let direct_artifact = compile_direct_artifact(
        &components_dir,
        "wait-signal-on-wait-error",
        WAIT_FOR_SIGNAL_DIRECT_ON_WAIT_ERROR,
    );
    let workflow_input = br#"{"case_id":"case-onwait-error","summary":"Notify failure"}"#;
    let components_input = components_sdk_input(workflow_input);

    let components = execute_artifact(
        &components_artifact,
        "ab-components-wait-signal-on-wait-error-0",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-wait-signal-on-wait-error-0",
        workflow_input,
    );

    assert_failure_parity("wait-signal-on-wait-error", 0, &components, &direct);
    assert_eq!(
        normalized_events(&direct.events)
            .iter()
            .map(|(subtype, _)| subtype.as_str())
            .collect::<Vec<_>>(),
        vec!["workflow_error"]
    );
}

#[test]
fn direct_wasm_matches_components_wait_for_signal_timeout() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact =
        compile_components_artifact("wait-signal-timeout", WAIT_FOR_SIGNAL_DIRECT_TIMEOUT);
    let direct_artifact = compile_direct_artifact(
        &components_dir,
        "wait-signal-timeout",
        WAIT_FOR_SIGNAL_DIRECT_TIMEOUT,
    );
    let workflow_input = br#"{"case_id":"case-timeout","summary":"No response"}"#;
    let components_input = components_sdk_input(workflow_input);

    let components = execute_artifact(
        &components_artifact,
        "ab-components-wait-signal-timeout-0",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-wait-signal-timeout-0",
        workflow_input,
    );

    assert_failure_parity("wait-signal-timeout", 0, &components, &direct);
}

#[test]
fn direct_wasm_matches_components_wait_for_signal_lifecycle_signals() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact =
        compile_components_artifact("wait-signal-lifecycle", WAIT_FOR_SIGNAL_DIRECT_SIMPLE);
    let direct_artifact = compile_direct_artifact(
        &components_dir,
        "wait-signal-lifecycle",
        WAIT_FOR_SIGNAL_DIRECT_SIMPLE,
    );
    let workflow_input = br#"{"case_id":"case-stop","summary":"Stop while waiting"}"#;
    let components_input = components_sdk_input(workflow_input);

    for (case_index, signal_type, expected_suspended_count) in [
        (0usize, "cancel", 0usize),
        (1, "pause", 1),
        (2, "shutdown", 1),
    ] {
        let components = execute_artifact_with_signal(
            &components_artifact,
            &format!("ab-components-wait-signal-{signal_type}"),
            &components_input,
            signal_type,
        );
        let direct = execute_artifact_with_signal(
            &direct_artifact.path,
            &format!("ab-direct-wait-signal-{signal_type}"),
            workflow_input,
            signal_type,
        );

        assert!(
            components.status_success,
            "components artifact did not stop cleanly for {signal_type}:\n{}",
            components.stderr
        );
        assert!(
            direct.status_success,
            "direct artifact did not stop cleanly for {signal_type}: {direct:?}"
        );
        assert!(
            components.output_json.is_none(),
            "components artifact unexpectedly completed for {signal_type}"
        );
        assert!(
            direct.output_json.is_none(),
            "direct artifact unexpectedly completed for {signal_type}"
        );
        assert!(
            components.error_json.is_none(),
            "components artifact unexpectedly failed for {signal_type}: {:?}",
            components.error_json
        );
        assert!(
            direct.error_json.is_none(),
            "direct artifact unexpectedly failed for {signal_type}: {:?}",
            direct.error_json
        );
        assert_eq!(
            normalized_events(&components.events),
            normalized_events(&direct.events),
            "wait lifecycle custom-event payload mismatch for {signal_type}"
        );
        assert_eq!(
            components.sleeps, direct.sleeps,
            "wait lifecycle sleep mismatch for {signal_type}"
        );
        assert_eq!(
            normalized_checkpoints(&components.checkpoints),
            normalized_checkpoints(&direct.checkpoints),
            "wait lifecycle checkpoint mismatch for {signal_type}"
        );
        assert_eq!(
            components.suspended_count, expected_suspended_count,
            "components suspended count mismatch for {signal_type}"
        );
        assert_eq!(
            direct.suspended_count, expected_suspended_count,
            "direct suspended count mismatch for {signal_type}"
        );
        let expected_ack = vec![SignalAckRequest {
            signal_type: signal_type.to_string(),
        }];
        assert_eq!(
            components.signal_acks, expected_ack,
            "components signal acknowledgement mismatch for {signal_type}"
        );
        assert_eq!(
            direct.signal_acks,
            vec![SignalAckRequest {
                signal_type: signal_type.to_string(),
            }],
            "direct signal acknowledgement mismatch for {signal_type}"
        );
        assert_eq!(
            components.signal_acks, direct.signal_acks,
            "signal acknowledgement parity mismatch for {signal_type}[{case_index}]"
        );
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
fn direct_wasm_matches_components_stop_after_durable_agent_checkpoint_signal() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact =
        compile_components_artifact("durable-agent-stop-signals", AGENT_RETURN_INPUT);
    let direct_artifact = compile_direct_artifact(
        &components_dir,
        "durable-agent-stop-signals",
        AGENT_RETURN_INPUT,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    for (signal_type, expected_suspended_count, resumes_from_checkpoint) in
        [("cancel", 0usize, false), ("shutdown", 1usize, true)]
    {
        let input_value = format!("agent-{signal_type}");
        let workflow_input = format!(r#"{{"value":"{input_value}"}}"#).into_bytes();
        let components_input = components_sdk_input(&workflow_input);
        let components_stopped = execute_artifact_with_checkpoint_signal(
            &components_artifact,
            &format!("ab-components-durable-agent-{signal_type}"),
            &components_input,
            signal_type,
        );
        let direct_stopped = execute_artifact_with_checkpoint_signal(
            &direct_artifact.path,
            &format!("ab-direct-durable-agent-{signal_type}"),
            &workflow_input,
            signal_type,
        );

        assert!(
            components_stopped.status_success,
            "components artifact did not stop cleanly for {signal_type}:\n{}",
            components_stopped.stderr
        );
        assert!(
            direct_stopped.status_success,
            "direct artifact did not stop cleanly for {signal_type}:\n{}",
            direct_stopped.stderr
        );
        assert!(
            components_stopped.output_json.is_none(),
            "components artifact unexpectedly completed for {signal_type}"
        );
        assert!(
            direct_stopped.output_json.is_none(),
            "direct artifact unexpectedly completed for {signal_type}"
        );
        assert!(
            components_stopped.error_json.is_none(),
            "components artifact unexpectedly failed for {signal_type}: {:?}",
            components_stopped.error_json
        );
        assert!(
            direct_stopped.error_json.is_none(),
            "direct artifact unexpectedly failed for {signal_type}: {:?}",
            direct_stopped.error_json
        );

        let saved_agent_output = format!(r#""{input_value}""#).into_bytes();
        let expected_checkpoint_traffic = vec![
            (AGENT_CACHE_KEY.to_string(), Vec::new()),
            (AGENT_CACHE_KEY.to_string(), saved_agent_output.clone()),
        ];
        assert_eq!(
            normalized_checkpoints(&components_stopped.checkpoints),
            expected_checkpoint_traffic
        );
        assert_eq!(
            normalized_checkpoints(&direct_stopped.checkpoints),
            expected_checkpoint_traffic
        );
        assert_eq!(
            components_stopped.suspended_count, expected_suspended_count,
            "components suspended count mismatch for {signal_type}"
        );
        assert_eq!(
            direct_stopped.suspended_count, expected_suspended_count,
            "direct suspended count mismatch for {signal_type}"
        );
        let expected_ack = vec![SignalAckRequest {
            signal_type: signal_type.to_string(),
        }];
        assert_eq!(components_stopped.signal_acks, expected_ack);
        assert_eq!(
            direct_stopped.signal_acks,
            vec![SignalAckRequest {
                signal_type: signal_type.to_string(),
            }]
        );

        if resumes_from_checkpoint {
            let components_resumed = execute_artifact_with_preloaded_checkpoints(
                &components_artifact,
                &format!("ab-components-durable-agent-{signal_type}-resume"),
                &components_input,
                vec![(AGENT_CACHE_KEY.to_string(), saved_agent_output.clone())],
            );
            let direct_resumed = execute_artifact_with_preloaded_checkpoints(
                &direct_artifact.path,
                &format!("ab-direct-durable-agent-{signal_type}-resume"),
                &workflow_input,
                vec![(AGENT_CACHE_KEY.to_string(), saved_agent_output)],
            );

            assert_success_parity(
                "durable-agent-stop-signals",
                0,
                &components_resumed,
                &direct_resumed,
            );

            let expected_output = serde_json::json!({ "result": input_value });
            assert_eq!(
                components_resumed.output_json.as_ref(),
                Some(&expected_output)
            );
            assert_eq!(direct_resumed.output_json.as_ref(), Some(&expected_output));
        }
    }
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
fn direct_wasm_matches_components_embed_workflow_static_child() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let child_workflows = embed_workflow_child_workflows();
    let components_artifact = compile_components_artifact_with_child_workflows(
        "embed-workflow-static-child",
        EMBED_WORKFLOW,
        &child_workflows,
    );
    let direct_artifact = compile_direct_artifact_with_child_workflows(
        &components_dir,
        "embed-workflow-static-child",
        EMBED_WORKFLOW,
        &child_workflows,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"input":"fresh-child"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-embed-workflow-static-child",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-embed-workflow-static-child",
        workflow_input,
    );

    assert_success_parity("embed-workflow-static-child", 0, &components, &direct);

    let expected_output = serde_json::json!({
        "result": { "result": "fresh-child" }
    });
    assert_eq!(components.output_json.as_ref(), Some(&expected_output));
    assert_eq!(direct.output_json.as_ref(), Some(&expected_output));

    let expected_step_result = serde_json::to_vec(&serde_json::json!({
        "stepId": "call_child",
        "stepName": "Unnamed",
        "stepType": "EmbedWorkflow",
        "childWorkflowId": "child_workflow",
        "outputs": { "result": "fresh-child" }
    }))
    .expect("checkpoint json");
    let expected_checkpoint_traffic = vec![
        (EMBED_WORKFLOW_CACHE_KEY.to_string(), Vec::new()),
        (EMBED_WORKFLOW_CACHE_KEY.to_string(), expected_step_result),
    ];
    assert_eq!(
        normalized_checkpoints(&components.checkpoints),
        expected_checkpoint_traffic
    );
    assert_eq!(
        normalized_checkpoints(&direct.checkpoints),
        expected_checkpoint_traffic
    );
}

#[test]
fn direct_wasm_matches_components_embed_workflow_breakpoint_pause_resume() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let graph_json = embed_workflow_breakpoint_parent_json();
    let child_workflows = embed_workflow_child_workflows();
    let components_artifact = compile_components_artifact_with_child_workflows(
        "embed-workflow-breakpoint",
        &graph_json,
        &child_workflows,
    );
    let direct_artifact = compile_direct_artifact_with_child_workflows(
        &components_dir,
        "embed-workflow-breakpoint",
        &graph_json,
        &child_workflows,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"input":"fresh-child"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components_paused = execute_artifact_with_debug_mode(
        &components_artifact,
        "ab-components-embed-workflow-breakpoint-pause",
        &components_input,
    );
    let direct_paused = execute_artifact_with_debug_mode(
        &direct_artifact.path,
        "ab-direct-embed-workflow-breakpoint-pause",
        workflow_input,
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
    assert!(components_paused.output_json.is_none());
    assert!(direct_paused.output_json.is_none());
    assert!(components_paused.error_json.is_none());
    assert!(direct_paused.error_json.is_none());

    let expected_checkpoint = vec![(
        "breakpoint::call_child".to_string(),
        br#""breakpoint_hit""#.to_vec(),
    )];
    assert_eq!(
        normalized_checkpoints(&components_paused.checkpoints),
        expected_checkpoint
    );
    assert_eq!(
        normalized_checkpoints(&direct_paused.checkpoints),
        expected_checkpoint
    );
    assert_eq!(
        normalized_events(&components_paused.events),
        normalized_events(&direct_paused.events)
    );
    let direct_pause_events = normalized_events(&direct_paused.events);
    let breakpoint_events = direct_pause_events
        .iter()
        .filter(|(subtype, _)| subtype == "breakpoint_hit")
        .collect::<Vec<_>>();
    assert_eq!(breakpoint_events.len(), 1);
    assert_eq!(breakpoint_events[0].1["step_type"], "EmbedWorkflow");
    assert_eq!(
        breakpoint_events[0].1["inputs"],
        serde_json::json!({ "childInput": "fresh-child" })
    );
    assert!(
        direct_pause_events
            .iter()
            .all(|(subtype, _)| subtype != "step_debug_start"),
        "first breakpoint hit should pause before EmbedWorkflow debug-start emission"
    );
    assert_eq!(components_paused.suspended_count, 1);
    assert_eq!(direct_paused.suspended_count, 1);
    let expected_pause_ack = vec![SignalAckRequest {
        signal_type: "pause".to_string(),
    }];
    assert_eq!(components_paused.signal_acks, expected_pause_ack);
    assert_eq!(direct_paused.signal_acks, expected_pause_ack);

    let components_resumed = execute_artifact_with_options(
        &components_artifact,
        "ab-components-embed-workflow-breakpoint-resume",
        &components_input,
        ExecuteOptions {
            preloaded_checkpoints: expected_checkpoint.clone(),
            debug_mode: true,
            ..ExecuteOptions::default()
        },
    );
    let direct_resumed = execute_artifact_with_options(
        &direct_artifact.path,
        "ab-direct-embed-workflow-breakpoint-resume",
        workflow_input,
        ExecuteOptions {
            preloaded_checkpoints: expected_checkpoint,
            debug_mode: true,
            ..ExecuteOptions::default()
        },
    );

    assert_success_parity(
        "embed-workflow-breakpoint-resume",
        0,
        &components_resumed,
        &direct_resumed,
    );
    assert_eq!(
        direct_resumed.output_json,
        Some(serde_json::json!({
            "result": { "result": "fresh-child" }
        }))
    );
    assert!(
        normalized_events(&direct_resumed.events)
            .iter()
            .all(|(subtype, _)| subtype != "breakpoint_hit"),
        "resume from breakpoint checkpoint should not emit a second breakpoint event"
    );
    assert_eq!(components_resumed.suspended_count, 0);
    assert_eq!(direct_resumed.suspended_count, 0);
    assert!(components_resumed.signal_acks.is_empty());
    assert!(direct_resumed.signal_acks.is_empty());
}

#[test]
fn direct_wasm_matches_components_cached_embed_workflow_checkpoint_replay() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let child_workflows = embed_workflow_child_workflows();
    let components_artifact = compile_components_artifact_with_child_workflows(
        "embed-workflow-cached",
        EMBED_WORKFLOW,
        &child_workflows,
    );
    let direct_artifact = compile_direct_artifact_with_child_workflows(
        &components_dir,
        "embed-workflow-cached",
        EMBED_WORKFLOW,
        &child_workflows,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"input":"fresh-child"}"#;
    let components_input = components_sdk_input(workflow_input);
    let cached_step_result = serde_json::to_vec(&serde_json::json!({
        "stepId": "call_child",
        "stepName": "Unnamed",
        "stepType": "EmbedWorkflow",
        "childWorkflowId": "child_workflow",
        "outputs": { "result": "cached-child" }
    }))
    .expect("checkpoint json");
    let components = execute_artifact_with_preloaded_checkpoints(
        &components_artifact,
        "ab-components-embed-workflow-cached",
        &components_input,
        vec![(
            EMBED_WORKFLOW_CACHE_KEY.to_string(),
            cached_step_result.clone(),
        )],
    );
    let direct = execute_artifact_with_preloaded_checkpoints(
        &direct_artifact.path,
        "ab-direct-embed-workflow-cached",
        workflow_input,
        vec![(EMBED_WORKFLOW_CACHE_KEY.to_string(), cached_step_result)],
    );

    assert_success_parity("embed-workflow-cached", 0, &components, &direct);

    let expected_output = serde_json::json!({
        "result": { "result": "cached-child" }
    });
    assert_eq!(components.output_json.as_ref(), Some(&expected_output));
    assert_eq!(direct.output_json.as_ref(), Some(&expected_output));

    let expected_lookup = vec![(EMBED_WORKFLOW_CACHE_KEY.to_string(), Vec::new())];
    assert_eq!(
        normalized_checkpoints(&components.checkpoints),
        expected_lookup
    );
    assert_eq!(normalized_checkpoints(&direct.checkpoints), expected_lookup);
}

#[test]
fn direct_wasm_matches_components_embed_workflow_terminal_error_child() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let child_workflows = embed_workflow_error_child_workflows();
    let components_artifact = compile_components_artifact_with_child_workflows(
        "embed-workflow-error-child",
        EMBED_WORKFLOW,
        &child_workflows,
    );
    let direct_artifact = compile_direct_artifact_with_child_workflows(
        &components_dir,
        "embed-workflow-error-child",
        EMBED_WORKFLOW,
        &child_workflows,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"input":"failing-child"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-embed-workflow-error-child",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-embed-workflow-error-child",
        workflow_input,
    );

    assert_failure_parity("embed-workflow-error-child", 0, &components, &direct);

    let expected_error = serde_json::json!({
        "stepId": "call_child",
        "stepName": "Unnamed",
        "stepType": "EmbedWorkflow",
        "category": "permanent",
        "code": "CHILD_WORKFLOW_FAILED",
        "message": "Child workflow child_workflow failed",
        "severity": "critical",
        "childWorkflowId": "child_workflow",
        "childError": {
            "stepId": "fail",
            "stepName": "Child Failure",
            "category": "permanent",
            "code": "CHILD_FAILED",
            "message": "Child workflow failed",
            "severity": "critical",
            "context": { "childInput": "failing-child" }
        }
    });
    assert_eq!(
        components.error_json.as_ref(),
        Some(&expected_error),
        "components terminal child Error payload changed"
    );
    assert_eq!(
        direct.error_json.as_ref(),
        Some(&expected_error),
        "direct terminal child Error payload changed"
    );

    let expected_lookup = vec![(EMBED_WORKFLOW_CACHE_KEY.to_string(), Vec::new())];
    assert_eq!(
        normalized_checkpoints(&components.checkpoints),
        expected_lookup
    );
    assert_eq!(normalized_checkpoints(&direct.checkpoints), expected_lookup);
}

#[test]
fn direct_wasm_matches_components_embed_workflow_parent_on_error() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let child_workflows = embed_workflow_error_child_workflows();
    let components_artifact = compile_components_artifact_with_child_workflows(
        "embed-workflow-parent-on-error",
        EMBED_WORKFLOW_ON_ERROR_PARENT,
        &child_workflows,
    );
    let direct_artifact = compile_direct_artifact_with_child_workflows(
        &components_dir,
        "embed-workflow-parent-on-error",
        EMBED_WORKFLOW_ON_ERROR_PARENT,
        &child_workflows,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"input":"failing-child"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-embed-workflow-parent-on-error",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-embed-workflow-parent-on-error",
        workflow_input,
    );

    assert_success_parity("embed-workflow-parent-on-error", 0, &components, &direct);

    let expected_output = serde_json::json!({
        "handled": true,
        "code": "CHILD_WORKFLOW_FAILED",
        "category": "permanent",
        "childCode": "CHILD_FAILED",
        "childStep": "fail"
    });
    assert_eq!(components.output_json.as_ref(), Some(&expected_output));
    assert_eq!(direct.output_json.as_ref(), Some(&expected_output));

    let expected_lookup = vec![(EMBED_WORKFLOW_CACHE_KEY.to_string(), Vec::new())];
    assert_eq!(
        normalized_checkpoints(&components.checkpoints),
        expected_lookup
    );
    assert_eq!(normalized_checkpoints(&direct.checkpoints), expected_lookup);
}

#[test]
fn direct_wasm_matches_components_embed_workflow_retry_exhausted() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let child_workflows = embed_workflow_transient_error_child_workflows();
    let components_artifact = compile_components_artifact_with_child_workflows(
        "embed-workflow-retry-exhausted",
        EMBED_WORKFLOW_RETRY_PARENT,
        &child_workflows,
    );
    let direct_artifact = compile_direct_artifact_with_child_workflows(
        &components_dir,
        "embed-workflow-retry-exhausted",
        EMBED_WORKFLOW_RETRY_PARENT,
        &child_workflows,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"input":"retry-child"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-embed-workflow-retry-exhausted",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-embed-workflow-retry-exhausted",
        workflow_input,
    );

    assert_failure_parity("embed-workflow-retry-exhausted", 0, &components, &direct);

    let expected_error = serde_json::json!({
        "stepId": "call_child",
        "stepName": "Unnamed",
        "stepType": "EmbedWorkflow",
        "category": "transient",
        "code": "CHILD_WORKFLOW_FAILED",
        "message": "Child workflow child_workflow failed",
        "severity": "error",
        "childWorkflowId": "child_workflow",
        "childError": {
            "stepId": "fail",
            "stepName": "Transient Child Failure",
            "category": "transient",
            "code": "CHILD_TEMPORARY",
            "message": "Child workflow failed transiently",
            "severity": "error",
            "context": { "childInput": "retry-child" }
        }
    });
    assert_eq!(components.error_json.as_ref(), Some(&expected_error));
    assert_eq!(direct.error_json.as_ref(), Some(&expected_error));

    let expected_lookup = vec![(EMBED_WORKFLOW_CACHE_KEY.to_string(), Vec::new())];
    assert_eq!(
        normalized_checkpoints(&components.checkpoints),
        expected_lookup
    );
    assert_eq!(normalized_checkpoints(&direct.checkpoints), expected_lookup);

    let expected_retry_attempts = vec![
        (
            EMBED_WORKFLOW_CACHE_KEY.to_string(),
            2,
            Some(expected_error.clone()),
        ),
        (
            EMBED_WORKFLOW_CACHE_KEY.to_string(),
            3,
            Some(expected_error),
        ),
    ];
    assert_eq!(
        normalized_retry_attempts(&components.retry_attempts),
        expected_retry_attempts
    );
    assert_eq!(
        normalized_retry_attempts(&direct.retry_attempts),
        expected_retry_attempts
    );
}

#[test]
fn direct_wasm_matches_components_split_retry_exhausted() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact =
        compile_components_artifact("split-retry-exhausted", SPLIT_RETRY_TRANSIENT_ERROR);
    let direct_artifact = compile_direct_artifact(
        &components_dir,
        "split-retry-exhausted",
        SPLIT_RETRY_TRANSIENT_ERROR,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"items":[{"value":"retry-item"}]}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-split-retry-exhausted",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-split-retry-exhausted",
        workflow_input,
    );

    assert_failure_parity("split-retry-exhausted", 0, &components, &direct);

    let expected_error = serde_json::json!({
        "stepId": "fail",
        "stepName": "Transient Item Failure",
        "category": "transient",
        "code": "SPLIT_ITEM_TEMPORARY",
        "message": "Split item failed transiently",
        "severity": "error",
        "context": {
            "item": "retry-item",
            "index": 0
        }
    });
    // Generated string-wraps the fatal item error; direct keeps it structured
    // (normalized for comparison — both carry the same SPLIT_ITEM_TEMPORARY).
    assert_eq!(
        normalized_failure_error(&components.error_json),
        Some(expected_error.clone())
    );
    assert_eq!(
        normalized_failure_error(&direct.error_json),
        Some(expected_error.clone())
    );

    let expected_lookup = vec![(SPLIT_CACHE_KEY.to_string(), Vec::new())];
    assert_eq!(
        normalized_checkpoints(&components.checkpoints),
        expected_lookup
    );
    assert_eq!(normalized_checkpoints(&direct.checkpoints), expected_lookup);

    let expected_retry_attempts = vec![
        (SPLIT_CACHE_KEY.to_string(), 2, Some(expected_error.clone())),
        (SPLIT_CACHE_KEY.to_string(), 3, Some(expected_error)),
    ];
    assert_eq!(
        normalized_retry_attempts(&components.retry_attempts),
        expected_retry_attempts
    );
    assert_eq!(
        normalized_retry_attempts(&direct.retry_attempts),
        expected_retry_attempts
    );
}

#[test]
fn direct_wasm_matches_components_nested_embed_workflow_retry_parent_frame_isolation() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let child_workflows = embed_workflow_nested_retry_child_workflows();
    let components_artifact = compile_components_artifact_with_child_workflows(
        "embed-workflow-nested-retry",
        EMBED_WORKFLOW_RETRY_PARENT,
        &child_workflows,
    );
    let direct_artifact = compile_direct_artifact_with_child_workflows(
        &components_dir,
        "embed-workflow-nested-retry",
        EMBED_WORKFLOW_RETRY_PARENT,
        &child_workflows,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"input":"nested-retry-child"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-embed-workflow-nested-retry",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-embed-workflow-nested-retry",
        workflow_input,
    );

    assert_failure_parity("embed-workflow-nested-retry", 0, &components, &direct);

    let expected_error = serde_json::json!({
        "stepId": "call_child",
        "stepName": "Unnamed",
        "stepType": "EmbedWorkflow",
        "category": "transient",
        "code": "CHILD_WORKFLOW_FAILED",
        "message": "Child workflow child_workflow failed",
        "severity": "error",
        "childWorkflowId": "child_workflow",
        "childError": {
            "stepId": "call_grandchild",
            "stepName": "Unnamed",
            "stepType": "EmbedWorkflow",
            "category": "transient",
            "code": "CHILD_WORKFLOW_FAILED",
            "message": "Child workflow grandchild_workflow failed",
            "severity": "error",
            "childWorkflowId": "grandchild_workflow",
            "childError": {
                "stepId": "fail_grandchild",
                "stepName": "Transient Grandchild Failure",
                "category": "transient",
                "code": "GRANDCHILD_TEMPORARY",
                "message": "Grandchild workflow failed transiently",
                "severity": "error",
                "context": { "grandchildInput": "nested-retry-child" }
            }
        }
    });
    assert_eq!(components.error_json.as_ref(), Some(&expected_error));
    assert_eq!(direct.error_json.as_ref(), Some(&expected_error));

    let components_retry_attempts = normalized_retry_attempts(&components.retry_attempts);
    let direct_retry_attempts = normalized_retry_attempts(&direct.retry_attempts);
    assert_eq!(direct_retry_attempts, components_retry_attempts);
    assert_eq!(
        direct_retry_attempts
            .iter()
            .map(|(key, attempt, _)| (key.as_str(), *attempt))
            .collect::<Vec<_>>(),
        vec![(EMBED_WORKFLOW_CACHE_KEY, 2), (EMBED_WORKFLOW_CACHE_KEY, 3)]
    );
    assert!(
        direct_retry_attempts
            .iter()
            .all(|(_, _, error)| { error.as_ref() == Some(&expected_error) })
    );
}

#[test]
fn direct_wasm_matches_components_embed_workflow_no_retry_transient_child() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let child_workflows = embed_workflow_transient_error_child_workflows();
    let components_artifact = compile_components_artifact_with_child_workflows(
        "embed-workflow-no-retry",
        EMBED_WORKFLOW_NO_RETRY_PARENT,
        &child_workflows,
    );
    let direct_artifact = compile_direct_artifact_with_child_workflows(
        &components_dir,
        "embed-workflow-no-retry",
        EMBED_WORKFLOW_NO_RETRY_PARENT,
        &child_workflows,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"input":"no-retry-child"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-embed-workflow-no-retry",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-embed-workflow-no-retry",
        workflow_input,
    );

    assert_failure_parity("embed-workflow-no-retry", 0, &components, &direct);
    assert!(components.retry_attempts.is_empty());
    assert!(direct.retry_attempts.is_empty());
}

#[test]
fn direct_wasm_matches_components_embed_workflow_parent_on_error_after_retry_exhausted() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let child_workflows = embed_workflow_transient_error_child_workflows();
    let components_artifact = compile_components_artifact_with_child_workflows(
        "embed-workflow-retry-on-error",
        EMBED_WORKFLOW_RETRY_ON_ERROR_PARENT,
        &child_workflows,
    );
    let direct_artifact = compile_direct_artifact_with_child_workflows(
        &components_dir,
        "embed-workflow-retry-on-error",
        EMBED_WORKFLOW_RETRY_ON_ERROR_PARENT,
        &child_workflows,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"input":"retry-on-error-child"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-embed-workflow-retry-on-error",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-embed-workflow-retry-on-error",
        workflow_input,
    );

    assert_success_parity("embed-workflow-retry-on-error", 0, &components, &direct);

    let expected_output = serde_json::json!({
        "handled": true,
        "code": "CHILD_WORKFLOW_FAILED",
        "category": "transient",
        "childCode": "CHILD_TEMPORARY",
        "childStep": "fail"
    });
    assert_eq!(components.output_json.as_ref(), Some(&expected_output));
    assert_eq!(direct.output_json.as_ref(), Some(&expected_output));

    assert_eq!(
        normalized_retry_attempts(&components.retry_attempts),
        normalized_retry_attempts(&direct.retry_attempts)
    );
    assert_eq!(
        normalized_retry_attempts(&direct.retry_attempts)
            .iter()
            .map(|(_, attempt, _)| *attempt)
            .collect::<Vec<_>>(),
        vec![2, 3]
    );
}

#[test]
fn direct_wasm_matches_components_embed_workflow_child_local_on_error() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let child_workflows = embed_workflow_child_local_on_error_child_workflows();
    let components_artifact = compile_components_artifact_with_child_workflows(
        "embed-workflow-child-local-on-error",
        EMBED_WORKFLOW_CHILD_LOCAL_ON_ERROR_PARENT,
        &child_workflows,
    );
    let direct_artifact = compile_direct_artifact_with_child_workflows(
        &components_dir,
        "embed-workflow-child-local-on-error",
        EMBED_WORKFLOW_CHILD_LOCAL_ON_ERROR_PARENT,
        &child_workflows,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"input":"child-local-on-error"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-embed-workflow-child-local-on-error",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-embed-workflow-child-local-on-error",
        workflow_input,
    );

    assert_success_parity(
        "embed-workflow-child-local-on-error",
        0,
        &components,
        &direct,
    );

    let expected_child_output = serde_json::json!({
        "handled": true,
        "code": "CHILD_WORKFLOW_FAILED",
        "category": "transient",
        "childCode": "GRANDCHILD_TEMPORARY",
        "childStep": "fail_grandchild",
        "childInput": "child-local-on-error"
    });
    let expected_child_step_result = serde_json::json!({
        "stepId": "call_child",
        "stepName": "Unnamed",
        "stepType": "EmbedWorkflow",
        "childWorkflowId": "child_workflow",
        "outputs": expected_child_output.clone()
    });
    let expected_output = serde_json::json!({
        "result": expected_child_output,
        "stepsSnapshot": {
            "call_child": expected_child_step_result
        }
    });
    assert_eq!(components.output_json.as_ref(), Some(&expected_output));
    assert_eq!(direct.output_json.as_ref(), Some(&expected_output));
    assert!(components.retry_attempts.is_empty());
    assert!(direct.retry_attempts.is_empty());
}

#[test]
fn direct_wasm_matches_components_embed_workflow_conditional_error_child() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let child_workflows = embed_workflow_conditional_error_child_workflows();
    let components_artifact = compile_components_artifact_with_child_workflows(
        "embed-workflow-conditional-error-child",
        EMBED_WORKFLOW,
        &child_workflows,
    );
    let direct_artifact = compile_direct_artifact_with_child_workflows(
        &components_dir,
        "embed-workflow-conditional-error-child",
        EMBED_WORKFLOW,
        &child_workflows,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let success_input = br#"{"input":"ok"}"#;
    let components_success_input = components_sdk_input(success_input);
    let components_success = execute_artifact(
        &components_artifact,
        "ab-components-embed-workflow-conditional-error-child-success",
        &components_success_input,
    );
    let direct_success = execute_artifact(
        &direct_artifact.path,
        "ab-direct-embed-workflow-conditional-error-child-success",
        success_input,
    );
    assert_success_parity(
        "embed-workflow-conditional-error-child-success",
        0,
        &components_success,
        &direct_success,
    );
    let expected_output = serde_json::json!({
        "result": { "result": "ok" }
    });
    assert_eq!(
        components_success.output_json.as_ref(),
        Some(&expected_output)
    );
    assert_eq!(direct_success.output_json.as_ref(), Some(&expected_output));
    let expected_step_result = serde_json::to_vec(&serde_json::json!({
        "stepId": "call_child",
        "stepName": "Unnamed",
        "stepType": "EmbedWorkflow",
        "childWorkflowId": "child_workflow",
        "outputs": { "result": "ok" }
    }))
    .expect("checkpoint json");
    let expected_checkpoint_traffic = vec![
        (EMBED_WORKFLOW_CACHE_KEY.to_string(), Vec::new()),
        (EMBED_WORKFLOW_CACHE_KEY.to_string(), expected_step_result),
    ];
    assert_eq!(
        normalized_checkpoints(&components_success.checkpoints),
        expected_checkpoint_traffic
    );
    assert_eq!(
        normalized_checkpoints(&direct_success.checkpoints),
        expected_checkpoint_traffic
    );

    let failure_input = br#"{"input":"failing-child"}"#;
    let components_failure_input = components_sdk_input(failure_input);
    let components_failure = execute_artifact(
        &components_artifact,
        "ab-components-embed-workflow-conditional-error-child-failure",
        &components_failure_input,
    );
    let direct_failure = execute_artifact(
        &direct_artifact.path,
        "ab-direct-embed-workflow-conditional-error-child-failure",
        failure_input,
    );
    assert_failure_parity(
        "embed-workflow-conditional-error-child-failure",
        0,
        &components_failure,
        &direct_failure,
    );
    let expected_error = serde_json::json!({
        "stepId": "call_child",
        "stepName": "Unnamed",
        "stepType": "EmbedWorkflow",
        "category": "permanent",
        "code": "CHILD_WORKFLOW_FAILED",
        "message": "Child workflow child_workflow failed",
        "severity": "critical",
        "childWorkflowId": "child_workflow",
        "childError": {
            "stepId": "fail",
            "stepName": "Conditional Child Failure",
            "category": "permanent",
            "code": "CONDITIONAL_CHILD_FAILED",
            "message": "Conditional child workflow failed",
            "severity": "critical",
            "context": { "childInput": "failing-child" }
        }
    });
    assert_eq!(
        components_failure.error_json.as_ref(),
        Some(&expected_error),
        "components conditional child Error payload changed"
    );
    assert_eq!(
        direct_failure.error_json.as_ref(),
        Some(&expected_error),
        "direct conditional child Error payload changed"
    );

    let expected_lookup = vec![(EMBED_WORKFLOW_CACHE_KEY.to_string(), Vec::new())];
    assert_eq!(
        normalized_checkpoints(&components_failure.checkpoints),
        expected_lookup
    );
    assert_eq!(
        normalized_checkpoints(&direct_failure.checkpoints),
        expected_lookup
    );
}

#[test]
fn direct_wasm_matches_components_nested_split_frame_isolation() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact = compile_components_artifact("nested-split", SPLIT_NESTED_SPLIT);
    let direct_artifact =
        compile_direct_artifact(&components_dir, "nested-split", SPLIT_NESTED_SPLIT);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input =
        br#"{"groups":[{"items":[{"value":"a1"},{"value":"a2"}]},{"items":[{"value":"b1"}]}]}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-nested-split",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-nested-split",
        workflow_input,
    );

    assert_success_parity("nested-split", 0, &components, &direct);
    let expected_output = serde_json::json!({
        "results": [
            {
                "outerIndex": 0,
                "outerIndices": [0],
                "inner": [
                    { "value": "a1", "innerIndex": 0, "indices": [0, 0] },
                    { "value": "a2", "innerIndex": 1, "indices": [0, 1] }
                ]
            },
            {
                "outerIndex": 1,
                "outerIndices": [1],
                "inner": [
                    { "value": "b1", "innerIndex": 0, "indices": [1, 0] }
                ]
            }
        ]
    });
    assert_eq!(components.output_json.as_ref(), Some(&expected_output));
    assert_eq!(direct.output_json.as_ref(), Some(&expected_output));
}

#[test]
fn direct_wasm_matches_components_while_with_nested_split_frame_isolation() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact = compile_components_artifact("while-nested-split", WHILE_NESTED_SPLIT);
    let direct_artifact =
        compile_direct_artifact(&components_dir, "while-nested-split", WHILE_NESTED_SPLIT);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"count":2,"items":[{"value":"x"},{"value":"y"}]}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-while-nested-split",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-while-nested-split",
        workflow_input,
    );

    assert_success_parity("while-nested-split", 0, &components, &direct);
    let expected_output = serde_json::json!({
        "iterations": 2,
        "last": {
            "loopIndex": 1,
            "loopIndices": [1],
            "inner": [
                { "value": "x", "splitIndex": 0, "indices": [1, 0] },
                { "value": "y", "splitIndex": 1, "indices": [1, 1] }
            ]
        }
    });
    assert_eq!(components.output_json.as_ref(), Some(&expected_output));
    assert_eq!(direct.output_json.as_ref(), Some(&expected_output));
}

#[test]
fn direct_wasm_matches_components_while_on_error() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact = compile_components_artifact("while-on-error", WHILE_ON_ERROR);
    let direct_artifact =
        compile_direct_artifact(&components_dir, "while-on-error", WHILE_ON_ERROR);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    // The loop body succeeds for index 0 and 1, then fails at index 2, so the
    // While step routes the captured failure to its onError handler.
    let workflow_input = br#"{}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-while-on-error",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-while-on-error",
        workflow_input,
    );

    assert_success_parity("while-on-error", 0, &components, &direct);
    assert_eq!(components.output_json, direct.output_json);

    let output = direct.output_json.as_ref().expect("direct onError output");
    assert_eq!(output.get("handled"), Some(&serde_json::json!(true)));
    assert_eq!(output.get("code"), Some(&serde_json::json!("WHILE_BOOM")));
    assert_eq!(
        output.get("category"),
        Some(&serde_json::json!("permanent"))
    );
}

/// Split onError is one place where direct mode intentionally does NOT match
/// generated Rust. Generated Rust wraps the failing item's error into a non-JSON
/// `"Split step '<id>' at iteration <n>: <e>"` string (codegen split.rs), so the
/// program-level onError wrapper's `serde_json::from_str` fails and `__error`
/// degrades to a generic `{code: null, category: "unknown"}` — losing the item's
/// structured error. Direct mode preserves the item's structured error, matching
/// how Agent onError already behaves. Per the parity goal we do not inherit that
/// inconsistency, so this test asserts direct's correct structured payload and
/// pins the generated-Rust degradation rather than asserting payload parity.
#[test]
fn direct_wasm_split_on_error_preserves_structured_error() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact = compile_components_artifact("split-on-error", SPLIT_ON_ERROR);
    let direct_artifact =
        compile_direct_artifact(&components_dir, "split-on-error", SPLIT_ON_ERROR);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    // The first item's body fails fatally (fail-fast Split), so the Split step
    // fails and routes the captured failure to its onError handler. Both artifacts
    // complete via the handler (no /failed); only the `__error` fidelity differs.
    let workflow_input = br#"{"items":[{"v":1}]}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-split-on-error",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-split-on-error",
        workflow_input,
    );

    let direct_out = direct.output_json.as_ref().expect("direct onError output");
    let components_out = components
        .output_json
        .as_ref()
        .expect("components onError output");

    // Direct preserves the item's structured error (correct, non-lossy).
    assert_eq!(direct_out.get("handled"), Some(&serde_json::json!(true)));
    assert_eq!(
        direct_out.get("code"),
        Some(&serde_json::json!("ITEM_BOOM"))
    );
    assert_eq!(
        direct_out.get("category"),
        Some(&serde_json::json!("permanent"))
    );

    // Pin the generated-Rust degradation we intentionally do not inherit; if
    // generated Rust is fixed to preserve the structured error, revisit this.
    assert_eq!(
        components_out.get("handled"),
        Some(&serde_json::json!(true))
    );
    assert_eq!(components_out.get("code"), Some(&serde_json::json!(null)));
    assert_eq!(
        components_out.get("category"),
        Some(&serde_json::json!("unknown"))
    );
}

/// An Agent step carrying a `compensation` (saga) config must behave exactly
/// like a plain agent: compensation is dead code end-to-end (codegen never
/// emits it, the SDK records `compensation_step_id: None`, the host
/// `CompensationManager` is never triggered), so generated Rust accepts and
/// ignores the field. Direct mode ungates it as the same no-op rather than
/// rejecting it, and this asserts full execution parity to prove the field is
/// inert in both paths.
#[test]
fn direct_wasm_matches_components_agent_compensation_noop() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact = compile_components_artifact("agent-compensation", AGENT_COMPENSATION);
    let direct_artifact =
        compile_direct_artifact(&components_dir, "agent-compensation", AGENT_COMPENSATION);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"value":{"hello":"world"}}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-agent-compensation",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-agent-compensation",
        workflow_input,
    );

    // Full parity: the compensation field changes nothing, so both artifacts
    // produce the identical return-input completion payload and event stream.
    assert_success_parity("agent-compensation", 0, &components, &direct);
    // Sanity that the agent actually ran (not an empty completion): the
    // return-input payload propagated into the finish output.
    let output_str =
        serde_json::to_string(direct.output_json.as_ref().expect("direct completion")).unwrap();
    assert!(
        output_str.contains("hello") && output_str.contains("world"),
        "agent output should carry the return-input payload: {output_str}"
    );
}

/// `AgentStep.timeout` is parsed but never enforced in the generated Rust path
/// (codegen never reads it; a synchronous `capabilities.invoke` cannot be
/// preempted). Generated accepts + ignores it, so direct ungates it as a no-op
/// instead of rejecting the workflow. Full execution parity proves inertness.
#[test]
fn direct_wasm_matches_components_agent_timeout_noop() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let graph = agent_timeout_json();
    let components_artifact = compile_components_artifact("agent-timeout", &graph);
    let direct_artifact = compile_direct_artifact(&components_dir, "agent-timeout", &graph);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"value":{"hello":"world"}}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-agent-timeout",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-agent-timeout",
        workflow_input,
    );

    assert_success_parity("agent-timeout", 0, &components, &direct);
    let output_str =
        serde_json::to_string(direct.output_json.as_ref().expect("direct completion")).unwrap();
    assert!(
        output_str.contains("hello") && output_str.contains("world"),
        "agent output should carry the return-input payload: {output_str}"
    );
}

/// `EmbedWorkflowStep.timeout` is likewise parsed but never enforced in the
/// generated Rust path (no child-run deadline exists). Direct ungates it as a
/// no-op; this asserts the embed call produces the identical output and
/// checkpoint traffic as the timeout-free static-child case.
#[test]
fn direct_wasm_matches_components_embed_workflow_timeout_noop() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let graph = embed_workflow_timeout_parent_json();
    let child_workflows = embed_workflow_child_workflows();
    let components_artifact = compile_components_artifact_with_child_workflows(
        "embed-workflow-timeout",
        &graph,
        &child_workflows,
    );
    let direct_artifact = compile_direct_artifact_with_child_workflows(
        &components_dir,
        "embed-workflow-timeout",
        &graph,
        &child_workflows,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"input":"fresh-child"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-embed-workflow-timeout",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-embed-workflow-timeout",
        workflow_input,
    );

    assert_success_parity("embed-workflow-timeout", 0, &components, &direct);
    let expected_output = serde_json::json!({ "result": { "result": "fresh-child" } });
    assert_eq!(components.output_json.as_ref(), Some(&expected_output));
    assert_eq!(direct.output_json.as_ref(), Some(&expected_output));
}

/// Single-shot AiAgent end-to-end parity. The direct artifact lowers the
/// AiAgent as an `ai-tools`/`chat-completion` invoke; the generated artifact
/// links runtara-ai inline. Both issue the LLM call through the in-test mock
/// proxy (`/proxy`), which returns a deterministic completion, so both produce
/// the identical `{response, iterations, toolCalls}` envelope.
#[test]
fn direct_wasm_matches_components_ai_agent_single_shot() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact =
        compile_components_artifact("ai-agent-single-shot", AI_AGENT_SINGLE_SHOT);
    let direct_artifact = compile_direct_artifact(
        &components_dir,
        "ai-agent-single-shot",
        AI_AGENT_SINGLE_SHOT,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"question":"What is 2+2?"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-ai-agent-single-shot",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-ai-agent-single-shot",
        workflow_input,
    );

    // Completion-payload parity: both paths call the same mock LLM and produce
    // the identical AiAgent output envelope. We deliberately do NOT assert
    // checkpoint-traffic parity here: the generated loop checkpoints the LLM
    // call under a bespoke `agent::<step>/llm/<iter>` key (storing the raw
    // choice), while direct mode reuses the standard Agent checkpoint
    // (`agent::ai-tools::chat-completion::<step>`, storing the capability's
    // `{choice, usage}`). Both are internally consistent for crash/resume; the
    // keys legitimately differ because the two lower the LLM call differently.
    assert!(
        components.status_success,
        "components AiAgent run failed:\n{}",
        components.stderr
    );
    assert!(
        direct.status_success,
        "direct AiAgent run failed:\n{}",
        direct.stderr
    );
    assert!(components.error_json.is_none() && direct.error_json.is_none());
    assert_eq!(
        components.output_json, direct.output_json,
        "AiAgent completion payload mismatch"
    );
    // The finish step maps `answer <- steps.ai.outputs.response`.
    let direct_out = direct.output_json.as_ref().expect("direct completion");
    assert_eq!(
        direct_out.get("answer"),
        Some(&serde_json::json!(MOCK_AI_RESPONSE))
    );
}

/// Structured-output AiAgent (config has an `outputSchema`). The mock returns
/// JSON content; both artifacts parse it, so the AiAgent `response` is the
/// parsed object (not a string) and the completion payloads match.
#[test]
fn direct_wasm_matches_components_ai_agent_structured_output() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact =
        compile_components_artifact("ai-agent-structured", AI_AGENT_STRUCTURED);
    let direct_artifact =
        compile_direct_artifact(&components_dir, "ai-agent-structured", AI_AGENT_STRUCTURED);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"text":"I love this!"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-ai-agent-structured",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-ai-agent-structured",
        workflow_input,
    );

    // Completion-payload parity (checkpoint traffic differs by design, see the
    // single-shot test).
    assert!(
        components.status_success,
        "components run failed:\n{}",
        components.stderr
    );
    assert!(
        direct.status_success,
        "direct run failed:\n{}",
        direct.stderr
    );
    assert_eq!(
        components.output_json, direct.output_json,
        "structured AiAgent completion payload mismatch"
    );
    // `response` is the parsed JSON object, mapped to `result` by finish.
    let direct_out = direct.output_json.as_ref().expect("direct completion");
    assert_eq!(
        direct_out.get("result"),
        Some(&serde_json::json!({ "sentiment": "positive", "confidence": 0.9 }))
    );
}

/// AiAgent with conversation memory. Both artifacts load history from the mock
/// object-model (empty), run a turn, and save — at output parity.
#[test]
fn direct_wasm_matches_components_ai_agent_memory() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact = compile_components_artifact("ai-agent-memory", AI_AGENT_MEMORY);
    let direct_artifact =
        compile_direct_artifact(&components_dir, "ai-agent-memory", AI_AGENT_MEMORY);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"q":"hi","session":"s-1"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-ai-agent-memory",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-ai-agent-memory",
        workflow_input,
    );

    assert!(
        components.status_success,
        "components run failed:\n{}\nerror={:?}",
        components.stderr, components.error_json
    );
    assert!(
        direct.status_success,
        "direct run failed:\nstderr={}\nerror={:?}",
        direct.stderr, direct.error_json
    );
    assert_eq!(
        components.output_json, direct.output_json,
        "memory AiAgent completion payload mismatch"
    );
    let direct_out = direct.output_json.as_ref().expect("direct completion");
    assert_eq!(
        direct_out.get("answer"),
        Some(&serde_json::json!(MOCK_AI_RESPONSE))
    );
}

/// AiAgent with memory + a tool + sliding-window compaction (maxMessages 2). The
/// tool loop accumulates four conversation messages; compaction drops the oldest
/// two before the memory save. Both artifacts complete at output parity AND
/// persist the identical compacted two-message conversation — the observable
/// effect of compaction (the completion payload alone cannot distinguish it).
#[test]
fn direct_wasm_matches_components_ai_agent_memory_compaction() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact =
        compile_components_artifact("ai-agent-memory-compaction", AI_AGENT_MEMORY_COMPACTION);
    let direct_artifact = compile_direct_artifact(
        &components_dir,
        "ai-agent-memory-compaction",
        AI_AGENT_MEMORY_COMPACTION,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"q":"hi","session":"s-compact"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-ai-agent-memory-compaction",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-ai-agent-memory-compaction",
        workflow_input,
    );

    assert!(
        components.status_success,
        "components run failed:\n{}\nerror={:?}",
        components.stderr, components.error_json
    );
    assert!(
        direct.status_success,
        "direct run failed:\nstderr={}\nerror={:?}",
        direct.stderr, direct.error_json
    );
    assert_eq!(
        components.output_json, direct.output_json,
        "memory-compaction AiAgent completion payload mismatch"
    );

    // Observable compaction: the direct run persists the conversation to the
    // object-model provider (reached via the composed object-model component →
    // RUNTARA_OBJECT_MODEL_URL), and sliding-window compaction caps it to the
    // two most recent messages. The four-message conversation produced by the
    // tool loop is `[user, assistant tool-call, user tool-result, assistant
    // text]`; compaction drops the oldest two, leaving the tool-result and the
    // final assistant text. (The generated path routes its memory provider
    // through a different transport in this harness, so cross-path save
    // comparison is not applicable; output parity is asserted above.)
    let direct_saved = direct
        .memory_saves
        .last()
        .expect("direct save-memory captured");
    assert_eq!(
        direct_saved.len(),
        2,
        "direct compaction should keep only the 2 most recent messages, got {}: {:?}",
        direct_saved.len(),
        direct_saved
    );
    // The kept messages are the most recent two: the tool result (a `user`
    // message carrying a `tool_result`) and the final `assistant` text.
    assert_eq!(direct_saved[0]["role"], serde_json::json!("user"));
    assert_eq!(direct_saved[1]["role"], serde_json::json!("assistant"));
    assert!(
        direct_saved[0]["content"]
            .as_array()
            .and_then(|content| content.first())
            .map(|first| first["type"] == serde_json::json!("tool_result"))
            .unwrap_or(false),
        "the oldest kept message should be the tool result, got {:?}",
        direct_saved[0]
    );
}

/// AiAgent with memory + a tool + Summarize-strategy compaction (maxMessages 2).
/// The tool loop produces four messages; Summarize compaction replaces the
/// oldest two with one LLM-generated `[Previous conversation summary]: …` user
/// message (the summarization runs through the `ai-tools` summarize-memory
/// capability → the same mock LLM proxy). The direct run persists exactly three
/// messages: the summary followed by the two most recent.
#[test]
fn direct_wasm_matches_components_ai_agent_memory_summarize() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact =
        compile_components_artifact("ai-agent-memory-summarize", AI_AGENT_MEMORY_SUMMARIZE);
    let direct_artifact = compile_direct_artifact(
        &components_dir,
        "ai-agent-memory-summarize",
        AI_AGENT_MEMORY_SUMMARIZE,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"q":"hi","session":"s-summarize"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-ai-agent-memory-summarize",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-ai-agent-memory-summarize",
        workflow_input,
    );

    assert!(
        components.status_success,
        "components run failed:\n{}\nerror={:?}",
        components.stderr, components.error_json
    );
    assert!(
        direct.status_success,
        "direct run failed:\nstderr={}\nerror={:?}",
        direct.stderr, direct.error_json
    );
    assert_eq!(
        components.output_json, direct.output_json,
        "memory-summarize AiAgent completion payload mismatch"
    );

    // Observable summarize compaction: of the four-message conversation, the
    // oldest two are replaced by a single summary message, leaving three saved.
    let direct_saved = direct
        .memory_saves
        .last()
        .expect("direct save-memory captured");
    assert_eq!(
        direct_saved.len(),
        3,
        "summarize should leave 1 summary + 2 recent messages, got {}: {:?}",
        direct_saved.len(),
        direct_saved
    );
    // The first message is the inserted summary (a `user` message whose text
    // begins with the summary marker).
    assert_eq!(direct_saved[0]["role"], serde_json::json!("user"));
    let summary_text = direct_saved[0]["content"]
        .as_array()
        .and_then(|content| content.first())
        .and_then(|first| first["text"].as_str())
        .unwrap_or_default();
    assert!(
        summary_text.starts_with("[Previous conversation summary]:"),
        "the first kept message should be the summary, got {:?}",
        direct_saved[0]
    );
}

/// AiAgent with an MCP toolset edge (`mcp.github`). The mock LLM drives the
/// three-step flow `github_search` → `github_invoke` → text; both synthetic
/// tools dispatch to the `mcp` agent (mcp-tool-search / mcp-tool-invoke) through
/// the proxy's JSON-RPC mock. Both artifacts complete at output parity.
#[test]
fn direct_wasm_matches_components_ai_agent_mcp() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact = compile_components_artifact("ai-agent-mcp", AI_AGENT_MCP);
    let direct_artifact = compile_direct_artifact(&components_dir, "ai-agent-mcp", AI_AGENT_MCP);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"q":"find an echo tool"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-ai-agent-mcp",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-ai-agent-mcp",
        workflow_input,
    );

    assert!(
        components.status_success,
        "components run failed:\n{}\nerror={:?}",
        components.stderr, components.error_json
    );
    assert!(
        direct.status_success,
        "direct run failed:\nstderr={}\nerror={:?}",
        direct.stderr, direct.error_json
    );
    assert_eq!(
        components.output_json, direct.output_json,
        "MCP AiAgent completion payload mismatch"
    );
    let direct_out = direct.output_json.as_ref().expect("direct completion");
    assert_eq!(
        direct_out.get("answer"),
        Some(&serde_json::json!(MOCK_AI_RESPONSE))
    );
}

/// A step fans out to two unconditional successors that rejoin at a single
/// Finish. The direct emitter linearizes the diamond topologically (start, left,
/// right, join) so each step runs once and the join sees BOTH branches' outputs
/// — at output parity with the generated path's topological execution.
#[test]
fn direct_wasm_matches_components_fanout_diamond() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact = compile_components_artifact("fanout-diamond", FANOUT_DIAMOND);
    let direct_artifact =
        compile_direct_artifact(&components_dir, "fanout-diamond", FANOUT_DIAMOND);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"q":"hello"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-fanout-diamond",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-fanout-diamond",
        workflow_input,
    );

    assert!(
        components.status_success,
        "components run failed:\n{}\nerror={:?}",
        components.stderr, components.error_json
    );
    assert!(
        direct.status_success,
        "direct run failed:\nstderr={}\nerror={:?}",
        direct.stderr, direct.error_json
    );
    assert_eq!(
        components.output_json, direct.output_json,
        "fan-out diamond completion payload mismatch"
    );
    // The join saw all three outputs: both fan-out branches plus the start.
    let direct_out = direct.output_json.as_ref().expect("direct completion");
    assert_eq!(
        direct_out.get("fromStart"),
        Some(&serde_json::json!("hello"))
    );
    assert_eq!(
        direct_out.get("fromLeft"),
        Some(&serde_json::json!("left-value"))
    );
    assert_eq!(
        direct_out.get("fromRight"),
        Some(&serde_json::json!("right-value"))
    );
}

/// AiAgent whose tool (utils/calculate) fails because the LLM calls it without
/// an `expression`. The tool error must be fed back to the model as the tool
/// result so the loop continues to a text answer — not fail the workflow. Both
/// artifacts complete at output parity (the direct path surfaces the structured
/// error envelope as the tool result; the generated path a `{"error":…}` blob —
/// invisible in the final completion payload).
#[test]
fn direct_wasm_matches_components_ai_agent_tool_error() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact =
        compile_components_artifact("ai-agent-tool-error", AI_AGENT_TOOL_ERROR);
    let direct_artifact =
        compile_direct_artifact(&components_dir, "ai-agent-tool-error", AI_AGENT_TOOL_ERROR);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"q":"not an expression"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-ai-agent-tool-error",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-ai-agent-tool-error",
        workflow_input,
    );

    // The key assertion: a tool error does NOT fail the workflow — both runs
    // recover and complete.
    assert!(
        components.status_success,
        "components run failed:\n{}\nerror={:?}",
        components.stderr, components.error_json
    );
    assert!(
        direct.status_success,
        "direct run should recover from the tool error, not fail:\nstderr={}\nerror={:?}",
        direct.stderr, direct.error_json
    );
    assert_eq!(
        components.output_json, direct.output_json,
        "tool-error AiAgent completion payload mismatch"
    );
    let direct_out = direct.output_json.as_ref().expect("direct completion");
    assert_eq!(
        direct_out.get("answer"),
        Some(&serde_json::json!(MOCK_AI_RESPONSE))
    );
}

/// AiAgent loop with two tools. The mock calls the last advertised tool, so the
/// direct loop dispatches by a non-zero tool index. Both artifacts complete at
/// output parity.
#[test]
fn direct_wasm_matches_components_ai_agent_multi_tool() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact =
        compile_components_artifact("ai-agent-multi-tool", AI_AGENT_MULTI_TOOL);
    let direct_artifact =
        compile_direct_artifact(&components_dir, "ai-agent-multi-tool", AI_AGENT_MULTI_TOOL);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"q":"do it"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-ai-agent-multi-tool",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-ai-agent-multi-tool",
        workflow_input,
    );

    assert!(
        components.status_success,
        "components run failed:\n{}\nerror={:?}",
        components.stderr, components.error_json
    );
    assert!(
        direct.status_success,
        "direct run failed:\nstderr={}\nerror={:?}",
        direct.stderr, direct.error_json
    );
    assert_eq!(
        components.output_json, direct.output_json,
        "multi-tool AiAgent completion payload mismatch"
    );
    let direct_out = direct.output_json.as_ref().expect("direct completion");
    assert_eq!(
        direct_out.get("answer"),
        Some(&serde_json::json!(MOCK_AI_RESPONSE))
    );
}

/// AiAgent tool loop. The mock returns a tool call on the first turn (the
/// request advertises a tool and the history is short), the direct loop
/// dispatches the `utils`/`return-input` tool back through the workflow, then
/// the mock returns text → complete. Both artifacts run the loop and produce
/// the identical final answer.
#[test]
fn direct_wasm_matches_components_ai_agent_tool_loop() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact = compile_components_artifact("ai-agent-tool-loop", AI_AGENT_TOOL_LOOP);
    let direct_artifact =
        compile_direct_artifact(&components_dir, "ai-agent-tool-loop", AI_AGENT_TOOL_LOOP);
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"q":"echo this"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-ai-agent-tool-loop",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-ai-agent-tool-loop",
        workflow_input,
    );

    assert!(
        components.status_success,
        "components run failed:\n{}\nerror={:?}",
        components.stderr, components.error_json
    );
    assert!(
        direct.status_success,
        "direct run failed:\nstderr={}\nerror={:?}\noutput={:?}",
        direct.stderr, direct.error_json, direct.output_json
    );
    assert_eq!(
        components.output_json, direct.output_json,
        "tool-loop AiAgent completion payload mismatch"
    );
    let direct_out = direct.output_json.as_ref().expect("direct completion");
    assert_eq!(
        direct_out.get("answer"),
        Some(&serde_json::json!(MOCK_AI_RESPONSE))
    );
}

#[test]
fn direct_wasm_matches_components_dont_stop_nested_split_failure_aggregation() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact = compile_components_artifact(
        "dont-stop-nested-split-error",
        SPLIT_DONT_STOP_NESTED_SPLIT_ERROR,
    );
    let direct_artifact = compile_direct_artifact(
        &components_dir,
        "dont-stop-nested-split-error",
        SPLIT_DONT_STOP_NESTED_SPLIT_ERROR,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"groups":[{"items":[{"value":"a"}]},{"items":[{"value":"b"}]}]}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-dont-stop-nested-split-error",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-dont-stop-nested-split-error",
        workflow_input,
    );

    assert_success_parity("dont-stop-nested-split-error", 0, &components, &direct);
    let output = direct.output_json.as_ref().expect("direct output");
    assert_eq!(output["outputs"], serde_json::json!([]));
    assert_eq!(output["stats"]["success"], serde_json::json!(0));
    assert_eq!(output["stats"]["error"], serde_json::json!(2));
    assert_eq!(output["stats"]["total"], serde_json::json!(2));
    assert_eq!(output["data"]["error"][0]["index"], serde_json::json!(0));
    assert_eq!(output["data"]["error"][1]["index"], serde_json::json!(1));
}

#[test]
fn direct_wasm_matches_components_dont_stop_deep_nested_failure_aggregation() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let components_artifact = compile_components_artifact(
        "dont-stop-deep-nested-error",
        SPLIT_DONT_STOP_DEEP_NESTED_WHILE_SPLIT_ERROR,
    );
    let direct_artifact = compile_direct_artifact(
        &components_dir,
        "dont-stop-deep-nested-error",
        SPLIT_DONT_STOP_DEEP_NESTED_WHILE_SPLIT_ERROR,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input =
        br#"{"groups":[{"count":1,"items":[{"value":"a"}]},{"count":1,"items":[{"value":"b"}]}]}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-dont-stop-deep-nested-error",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-dont-stop-deep-nested-error",
        workflow_input,
    );

    assert_success_parity("dont-stop-deep-nested-error", 0, &components, &direct);
    let output = direct.output_json.as_ref().expect("direct output");
    assert_eq!(output["outputs"], serde_json::json!([]));
    assert_eq!(output["stats"]["success"], serde_json::json!(0));
    assert_eq!(output["stats"]["error"], serde_json::json!(2));
    assert_eq!(output["stats"]["total"], serde_json::json!(2));
    assert_eq!(output["data"]["error"][0]["index"], serde_json::json!(0));
    assert_eq!(output["data"]["error"][1]["index"], serde_json::json!(1));
}

#[test]
fn direct_wasm_matches_components_nested_embed_workflow_static_child_closure() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let child_workflows = embed_workflow_nested_child_workflows();
    let components_artifact = compile_components_artifact_with_child_workflows(
        "embed-workflow-nested",
        EMBED_WORKFLOW_NESTED_PARENT,
        &child_workflows,
    );
    let direct_artifact = compile_direct_artifact_with_child_workflows(
        &components_dir,
        "embed-workflow-nested",
        EMBED_WORKFLOW_NESTED_PARENT,
        &child_workflows,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"input":"nested-child"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-embed-workflow-nested",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-embed-workflow-nested",
        workflow_input,
    );

    assert_success_parity("embed-workflow-nested", 0, &components, &direct);

    let expected_child_step_result = serde_json::json!({
        "stepId": "call_child",
        "stepName": "Unnamed",
        "stepType": "EmbedWorkflow",
        "childWorkflowId": "child_workflow",
        "outputs": { "result": "nested-child" }
    });
    let expected_output = serde_json::json!({
        "result": { "result": "nested-child" },
        "stepsSnapshot": {
            "call_child": expected_child_step_result
        }
    });
    assert_eq!(components.output_json.as_ref(), Some(&expected_output));
    assert_eq!(direct.output_json.as_ref(), Some(&expected_output));

    let component_checkpoints = normalized_checkpoints(&components.checkpoints);
    let direct_checkpoints = normalized_checkpoints(&direct.checkpoints);
    assert_eq!(direct_checkpoints, component_checkpoints);
    assert_eq!(
        component_checkpoints
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>(),
        vec![
            "embed_workflow::call_child",
            "embed_workflow::call_grandchild",
            "embed_workflow::call_greatgrandchild",
            "embed_workflow::call_greatgrandchild",
            "embed_workflow::call_grandchild",
            "embed_workflow::call_child",
        ]
    );
}

#[test]
fn direct_wasm_matches_components_nested_embed_workflow_failure_closure() {
    let Some(components_dir) = direct_ab_components_dir() else {
        return;
    };
    let _data = setup_data_dir();

    let child_workflows = embed_workflow_nested_error_child_workflows();
    let components_artifact = compile_components_artifact_with_child_workflows(
        "embed-workflow-nested-error",
        EMBED_WORKFLOW_NESTED_PARENT,
        &child_workflows,
    );
    let direct_artifact = compile_direct_artifact_with_child_workflows(
        &components_dir,
        "embed-workflow-nested-error",
        EMBED_WORKFLOW_NESTED_PARENT,
        &child_workflows,
    );
    assert_eq!(
        direct_artifact.compiler_mode,
        WorkflowCompilerMode::DirectWasm
    );

    let workflow_input = br#"{"input":"nested-child"}"#;
    let components_input = components_sdk_input(workflow_input);
    let components = execute_artifact(
        &components_artifact,
        "ab-components-embed-workflow-nested-error",
        &components_input,
    );
    let direct = execute_artifact(
        &direct_artifact.path,
        "ab-direct-embed-workflow-nested-error",
        workflow_input,
    );

    assert_failure_parity("embed-workflow-nested-error", 0, &components, &direct);

    let expected_error = serde_json::json!({
        "stepId": "call_child",
        "stepName": "Unnamed",
        "stepType": "EmbedWorkflow",
        "category": "permanent",
        "code": "CHILD_WORKFLOW_FAILED",
        "message": "Child workflow child_workflow failed",
        "severity": "critical",
        "childWorkflowId": "child_workflow",
        "childError": {
            "stepId": "call_grandchild",
            "stepName": "Unnamed",
            "stepType": "EmbedWorkflow",
            "category": "permanent",
            "code": "CHILD_WORKFLOW_FAILED",
            "message": "Child workflow grandchild_workflow failed",
            "severity": "critical",
            "childWorkflowId": "grandchild_workflow",
            "childError": {
                "stepId": "call_greatgrandchild",
                "stepName": "Unnamed",
                "stepType": "EmbedWorkflow",
                "category": "permanent",
                "code": "CHILD_WORKFLOW_FAILED",
                "message": "Child workflow great_grandchild_workflow failed",
                "severity": "critical",
                "childWorkflowId": "great_grandchild_workflow",
                "childError": {
                    "stepId": "fail_great_grandchild",
                    "stepName": "Great Grandchild Failure",
                    "category": "permanent",
                    "code": "GREAT_GRANDCHILD_FAILED",
                    "message": "Great grandchild workflow failed",
                    "severity": "critical",
                    "context": { "greatGrandchildInput": "nested-child" }
                }
            }
        }
    });
    assert_eq!(components.error_json.as_ref(), Some(&expected_error));
    assert_eq!(direct.error_json.as_ref(), Some(&expected_error));

    let component_checkpoints = normalized_checkpoints(&components.checkpoints);
    let direct_checkpoints = normalized_checkpoints(&direct.checkpoints);
    assert_eq!(direct_checkpoints, component_checkpoints);
    assert_eq!(
        component_checkpoints
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>(),
        vec![
            "embed_workflow::call_child",
            "embed_workflow::call_grandchild",
            "embed_workflow::call_greatgrandchild",
        ]
    );
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
