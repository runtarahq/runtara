// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Direct workflow compilation entry point — the only compile path.
//!
//! Orchestrates the whole DSL-graph -> core-Wasm -> composed-component pipeline.
//! `compile_direct_workflow` builds the manifest, runs the support gate
//! (hard-failing with a per-feature report on any unsupported shape — there is
//! no fallback compiler), then emits the core
//! module byte-by-byte and lifts it into a component via `wit_component`,
//! appending the manifest/support/ABI JSON as custom sections.
//! `compose_direct_workflow` is the separate second phase that composes that
//! `workflow-logic.wasm` against the prebuilt shared + per-agent components into
//! the runnable `workflow.wasm` (so the emitted logic is an inspectable artifact
//! before the compose step). Composition runs in-process through the
//! `wac-parser`/`wac-resolver`/`wac-graph` crates — no external `wac` binary.
//!
//! This file also owns the bank of `DIRECT_*` constants: the hand-assigned Wasm
//! local-variable slots and Canonical-ABI struct/offset layout that every per-step
//! lowerer in `compile/*` shares. There is no `rustc` here to allocate locals or
//! compute struct layouts, so the emitter fixes them once and all lowerers agree;
//! the deliberate slot aliasing (e.g. While reusing Split slots) encodes that
//! mutually-exclusive control-flow constructs can safely share scratch registers.

mod abi;
mod agent;
mod agent_error;
mod agent_invoke;
mod agent_io;
mod agent_retry;
mod ai_agent_loop;
mod artifact_metadata;
mod checkpoint;
mod core_imports;
mod core_module;
mod debug;
mod delay;
mod dispatcher;
mod edge_route;
mod embed_retry;
mod embed_workflow;
mod error_step;
mod log;
mod mapping;
mod split;
mod split_retry;
mod step_context;
mod step_error;
mod switch_route;
mod wait;
mod while_loop;

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use runtara_dsl::ExecutionGraph;
use runtara_workflow_wit::{
    LIFECYCLE_INTERFACE_NAME, LIFECYCLE_WIT, RUNTIME_WIT, STDLIB_WIT, WORKFLOW_WIT_VERSION,
};
use sha2::{Digest, Sha256};
use wasm_encoder::{CustomSection, Encode, Function as WasmFunction, Instruction, Section};
use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};
use wit_parser::{Resolve, WorldId};

pub use super::child_workflows::DirectChildWorkflowDependencyMetadata;
use super::child_workflows::resolve_direct_child_workflow_metadata;
use abi::push_retptr_arg;
pub use artifact_metadata::{
    DirectArtifactFileMetadata, DirectArtifactMetadata, DirectComponentDependencyMetadata,
    DirectComponentSidecarMetadata,
};
use artifact_metadata::{
    InitialArtifactMetadataInput, initial_artifact_metadata, resolve_agent_component_dependencies,
    resolve_shared_component_dependencies, write_artifact_metadata,
};
use core_imports::{DirectAgentInvokeImport, DirectCoreFunctionIndices};
use core_module::{DirectCoreConfig, DirectVariables, emit_direct_core_module};

use super::component::{DIRECT_AGENT_WIT_VERSION, DirectComponentArtifacts};
use super::error::DirectCompileError;
use super::manifest::{
    DIRECT_WORKFLOW_MANIFEST_VERSION, DirectManifestChildWorkflowInput, DirectWorkflowManifest,
    build_direct_workflow_manifest_with_child_workflows_and_agent_catalog,
};
use super::plan::{
    DirectEdgeConditionPlan, DirectErrorRoutePlan, DirectFailureTarget, DirectHandledTarget,
    DirectRunPlan, DirectSwitchRoutePlan, direct_run_plan,
};
#[cfg(test)]
use super::static_data::{
    DIRECT_AGENT_RATE_LIMIT_WAIT, DIRECT_STEP_DEBUG_END_KIND, DIRECT_STEP_DEBUG_START_KIND,
    DIRECT_WORKFLOW_ERROR_KIND, DIRECT_WORKFLOW_LOG_KIND,
};
use super::static_data::{
    DIRECT_EMPTY_STEPS_CONTEXT, DirectCoreStaticData, DirectDataSegment, WASM_PAGE_SIZE,
    direct_core_variables_json,
};
use super::support::{
    DirectWorkflowSupportReport, analyze_direct_wasm_support_with_child_workflows,
};

/// Direct workflow artifact ABI version (`wasi:cli/run` export shape).
pub const DIRECT_WORKFLOW_ABI_VERSION: u32 = 1;
/// Direct workflow artifact ABI version for the unified invoke export
/// (`runtara:workflow-lifecycle/lifecycle.invoke`).
pub const DIRECT_WORKFLOW_INVOKE_ABI_VERSION: u32 = 2;
/// Custom section containing [`DirectWorkflowManifest`] JSON.
pub const DIRECT_WORKFLOW_MANIFEST_SECTION: &str = "runtara.direct_workflow.manifest";
/// Custom section containing [`DirectWorkflowSupportReport`] JSON.
pub const DIRECT_WORKFLOW_SUPPORT_SECTION: &str = "runtara.direct_workflow.support";
/// Custom section containing direct artifact ABI metadata JSON.
pub const DIRECT_WORKFLOW_ABI_SECTION: &str = "runtara.direct_workflow.abi";
/// Version for `artifact-metadata.json` emitted beside direct artifacts.
pub const DIRECT_WORKFLOW_ARTIFACT_METADATA_VERSION: u32 = 2;
/// Sidecar filename containing direct artifact dependency/provenance metadata.
pub const DIRECT_WORKFLOW_ARTIFACT_METADATA_FILENAME: &str = "artifact-metadata.json";

const WASI_CLI_RUN_WIT: &str = r#"
package wasi:cli@0.2.3;

interface run {
    run: func() -> result;
}

world command {
    export run;
}
"#;
const AGENT_TYPES_WIT: &str = include_str!("../../../runtara-agent-wit/wit/runtara-agent.wit");
const AGENT_WIT_VERSION: &str = DIRECT_AGENT_WIT_VERSION;

const DIRECT_RUN_RETPTR_OFFSET: i32 = 0;
const DIRECT_RET_BOOL_OK_OFFSET: u64 = 4;
const DIRECT_RET_U32_OK_OFFSET: u64 = 4;
const DIRECT_RET_U64_OK_OFFSET: u64 = 8;
// Canonical ABI aligns option payloads by inner type: option<list<u8>> keeps
// the option tag at 4, while option<u64> aligns its tag/value to 8-byte slots.
const DIRECT_RESULT_OPTION_TAG_OFFSET: u64 = 4;
const DIRECT_RESULT_OPTION_U64_TAG_OFFSET: u64 = 8;
const DIRECT_RESULT_OPTION_U64_VALUE_OFFSET: u64 = 16;
const DIRECT_RESULT_OPTION_LIST_PTR_OFFSET: u64 = 8;
const DIRECT_RESULT_OPTION_LIST_LEN_OFFSET: u64 = 12;
const DIRECT_CHECKPOINT_FOUND_OFFSET: u64 = 4;
const DIRECT_CHECKPOINT_PENDING_SIGNAL_TAG_OFFSET: u64 = 16;
const DIRECT_CHECKPOINT_SIGNAL_TYPE_PTR_OFFSET: u64 = 20;
const DIRECT_CHECKPOINT_SIGNAL_TYPE_LEN_OFFSET: u64 = 24;
const DIRECT_AGENT_ARGS_OFFSET: i32 = 128;
const DIRECT_AGENT_ARG_CONNECTION_TAG_OFFSET: i32 = DIRECT_AGENT_ARGS_OFFSET + 16;
const DIRECT_AGENT_ARG_CONNECTION_ID_PTR_OFFSET: i32 = DIRECT_AGENT_ARGS_OFFSET + 20;
const DIRECT_AGENT_ARG_CONNECTION_ID_LEN_OFFSET: i32 = DIRECT_AGENT_ARGS_OFFSET + 24;
const DIRECT_AGENT_ARG_CONNECTION_INTEGRATION_PTR_OFFSET: i32 = DIRECT_AGENT_ARGS_OFFSET + 28;
const DIRECT_AGENT_ARG_CONNECTION_INTEGRATION_LEN_OFFSET: i32 = DIRECT_AGENT_ARGS_OFFSET + 32;
const DIRECT_AGENT_ARG_CONNECTION_SUBTYPE_TAG_OFFSET: i32 = DIRECT_AGENT_ARGS_OFFSET + 36;
const DIRECT_AGENT_ARG_CONNECTION_PARAMETERS_PTR_OFFSET: i32 = DIRECT_AGENT_ARGS_OFFSET + 48;
const DIRECT_AGENT_ARG_CONNECTION_PARAMETERS_LEN_OFFSET: i32 = DIRECT_AGENT_ARGS_OFFSET + 52;
const DIRECT_AGENT_ARG_CONNECTION_RATE_LIMIT_TAG_OFFSET: i32 = DIRECT_AGENT_ARGS_OFFSET + 56;
/// Fixed 8-byte scratch slot in the reserved 256-byte low-memory region (past
/// the retptr scratch at 0 and the agent-args scratch at 128, below the static
/// data base at 256). Used to marshal a `WaitForSignal` absolute timeout
/// deadline (i64 ms since epoch) into the bytes of its durability checkpoint so
/// a resumed wait reads the original deadline instead of recomputing
/// `now + timeout` and sliding it forward on every replay.
const DIRECT_WAIT_DEADLINE_SCRATCH_OFFSET: i32 = 208;
const DIRECT_AGENT_RESULT_OK_PTR_OFFSET: u64 = 8;
const DIRECT_AGENT_RESULT_OK_LEN_OFFSET: u64 = 12;
const DIRECT_AGENT_RESULT_ERR_CODE_PTR_OFFSET: u64 = 8;
const DIRECT_AGENT_RESULT_ERR_CODE_LEN_OFFSET: u64 = 12;
const DIRECT_AGENT_RESULT_ERR_MESSAGE_PTR_OFFSET: u64 = 16;
const DIRECT_AGENT_RESULT_ERR_MESSAGE_LEN_OFFSET: u64 = 20;
const DIRECT_AGENT_RESULT_ERR_CATEGORY_PTR_OFFSET: u64 = 24;
const DIRECT_AGENT_RESULT_ERR_CATEGORY_LEN_OFFSET: u64 = 28;
const DIRECT_AGENT_RESULT_ERR_SEVERITY_PTR_OFFSET: u64 = 32;
const DIRECT_AGENT_RESULT_ERR_SEVERITY_LEN_OFFSET: u64 = 36;
const DIRECT_AGENT_RESULT_ERR_RETRYABLE_OFFSET: u64 = 40;
const DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_TAG_OFFSET: u64 = 48;
const DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_VALUE_OFFSET: u64 = 56;
const DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_TAG_OFFSET: u64 = 64;
const DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_PTR_OFFSET: u64 = 68;
const DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_LEN_OFFSET: u64 = 72;
const DIRECT_AGENT_RETRY_INFO_PAYLOAD_PTR_OFFSET: u64 = 4;
const DIRECT_AGENT_RETRY_INFO_PAYLOAD_LEN_OFFSET: u64 = 8;
const DIRECT_AGENT_RETRY_INFO_RETRYABLE_OFFSET: u64 = 12;
const DIRECT_AGENT_RETRY_INFO_RATE_LIMITED_OFFSET: u64 = 13;
const DIRECT_AGENT_RETRY_ATTEMPT_LOCAL: u32 = 10;
const DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL: u32 = 11;
const DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL: u32 = 12;
const DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL: u32 = 13;
const DIRECT_AGENT_RETRYABLE_LOCAL: u32 = 14;
const DIRECT_AGENT_RATE_LIMITED_LOCAL: u32 = 15;
const DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL: u32 = 16;
const DIRECT_AGENT_RATE_LIMIT_WAIT_TOTAL_LOCAL: u32 = 17;
const DIRECT_DELAY_DURATION_MS_LOCAL: u32 = DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL;
const DIRECT_WAIT_TIMEOUT_PRESENT_LOCAL: u32 = DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL;
const DIRECT_WAIT_POLL_INTERVAL_MS_LOCAL: u32 = DIRECT_DELAY_DURATION_MS_LOCAL;
const DIRECT_WAIT_DEADLINE_MS_LOCAL: u32 = DIRECT_AGENT_RATE_LIMIT_WAIT_TOTAL_LOCAL;
const DIRECT_SPLIT_COUNT_LOCAL: u32 = 18;
const DIRECT_SPLIT_INDEX_LOCAL: u32 = 19;
const DIRECT_SPLIT_ITEM_PTR_LOCAL: u32 = 20;
const DIRECT_SPLIT_ITEM_LEN_LOCAL: u32 = 21;
const DIRECT_SPLIT_RESULTS_PTR_LOCAL: u32 = 22;
const DIRECT_SPLIT_RESULTS_LEN_LOCAL: u32 = 23;
const DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL: u32 = 24;
const DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL: u32 = 25;
const DIRECT_SPLIT_VARIABLES_PTR_LOCAL: u32 = 26;
const DIRECT_SPLIT_VARIABLES_LEN_LOCAL: u32 = 27;
const DIRECT_WHILE_MAX_ITERATIONS_LOCAL: u32 = DIRECT_SPLIT_COUNT_LOCAL;
const DIRECT_WHILE_INDEX_LOCAL: u32 = DIRECT_SPLIT_INDEX_LOCAL;
const DIRECT_WHILE_STATE_PTR_LOCAL: u32 = DIRECT_SPLIT_RESULTS_PTR_LOCAL;
const DIRECT_WHILE_STATE_LEN_LOCAL: u32 = DIRECT_SPLIT_RESULTS_LEN_LOCAL;
const DIRECT_WHILE_PARENT_SOURCE_PTR_LOCAL: u32 = DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL;
const DIRECT_WHILE_PARENT_SOURCE_LEN_LOCAL: u32 = DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL;
const DIRECT_WHILE_VARIABLES_PTR_LOCAL: u32 = DIRECT_SPLIT_VARIABLES_PTR_LOCAL;
const DIRECT_WHILE_VARIABLES_LEN_LOCAL: u32 = DIRECT_SPLIT_VARIABLES_LEN_LOCAL;
const DIRECT_WAIT_TIMEOUT_MS_LOCAL: u32 = 28;
const DIRECT_WAIT_ON_WAIT_VARIABLES_PTR_LOCAL: u32 = 29;
const DIRECT_WAIT_ON_WAIT_VARIABLES_LEN_LOCAL: u32 = 30;
const DIRECT_WAIT_PARENT_STEPS_PTR_LOCAL: u32 = 31;
const DIRECT_WAIT_PARENT_STEPS_LEN_LOCAL: u32 = 32;
const DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL: u32 = 33;
const DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL: u32 = 34;
const DIRECT_EMBED_CHILD_DATA_PTR_LOCAL: u32 = 35;
const DIRECT_EMBED_CHILD_DATA_LEN_LOCAL: u32 = 36;
const DIRECT_EMBED_CHILD_VARIABLES_PTR_LOCAL: u32 = 37;
const DIRECT_EMBED_CHILD_VARIABLES_LEN_LOCAL: u32 = 38;
const DIRECT_EMBED_PARENT_SOURCE_PTR_LOCAL: u32 = 39;
const DIRECT_EMBED_PARENT_SOURCE_LEN_LOCAL: u32 = 40;
const DIRECT_EMBED_STEP_RESULT_PTR_LOCAL: u32 = 41;
const DIRECT_EMBED_STEP_RESULT_LEN_LOCAL: u32 = 42;
const DIRECT_EMBED_CHILD_ERROR_PTR_LOCAL: u32 = 43;
const DIRECT_EMBED_CHILD_ERROR_LEN_LOCAL: u32 = 44;
const DIRECT_EMBED_CHILD_ERROR_FLAG_LOCAL: u32 = 45;
const DIRECT_EMBED_RETRY_ATTEMPT_LOCAL: u32 = 46;
const DIRECT_EMBED_RETRYABLE_LOCAL: u32 = 47;
const DIRECT_EMBED_RATE_LIMITED_LOCAL: u32 = 48;
const DIRECT_EMBED_RETRY_AFTER_TAG_LOCAL: u32 = 49;
const DIRECT_EMBED_RETRY_SLEEP_KEY_PTR_LOCAL: u32 = 50;
const DIRECT_EMBED_RETRY_SLEEP_KEY_LEN_LOCAL: u32 = 51;
const DIRECT_EMBED_RETRY_SLEEP_MS_LOCAL: u32 = 52;
const DIRECT_EMBED_RATE_LIMIT_WAIT_TOTAL_LOCAL: u32 = 53;
const DIRECT_SPLIT_FAILURE_COUNT_LOCAL: u32 = 54;
const DIRECT_SPLIT_FAILURE_INDEX_LOCAL: u32 = 55;
const DIRECT_SPLIT_FAILURE_ITEM_PTR_LOCAL: u32 = 56;
const DIRECT_SPLIT_FAILURE_ITEM_LEN_LOCAL: u32 = 57;
const DIRECT_SPLIT_FAILURE_RESULTS_PTR_LOCAL: u32 = 58;
const DIRECT_SPLIT_FAILURE_RESULTS_LEN_LOCAL: u32 = 59;
const DIRECT_SPLIT_FAILURE_PARENT_SOURCE_PTR_LOCAL: u32 = 60;
const DIRECT_SPLIT_FAILURE_PARENT_SOURCE_LEN_LOCAL: u32 = 61;
const DIRECT_SPLIT_FAILURE_VARIABLES_PTR_LOCAL: u32 = 62;
const DIRECT_SPLIT_FAILURE_VARIABLES_LEN_LOCAL: u32 = 63;
const DIRECT_SPLIT_RETRY_ERROR_FLAG_LOCAL: u32 = 64;
const DIRECT_SPLIT_RETRY_ATTEMPT_LOCAL: u32 = 65;
const DIRECT_SPLIT_RETRYABLE_LOCAL: u32 = 66;
const DIRECT_SPLIT_RATE_LIMITED_LOCAL: u32 = 67;
const DIRECT_SPLIT_RETRY_AFTER_TAG_LOCAL: u32 = 68;
const DIRECT_SPLIT_RETRY_SLEEP_KEY_PTR_LOCAL: u32 = 69;
const DIRECT_SPLIT_RETRY_SLEEP_KEY_LEN_LOCAL: u32 = 70;
const DIRECT_SPLIT_RETRY_ERROR_PTR_LOCAL: u32 = 71;
const DIRECT_SPLIT_RETRY_ERROR_LEN_LOCAL: u32 = 72;
const DIRECT_SPLIT_RETRY_SLEEP_MS_LOCAL: u32 = 73;
const DIRECT_SPLIT_RATE_LIMIT_WAIT_TOTAL_LOCAL: u32 = 74;
const DIRECT_STEP_ERROR_FLAG_LOCAL: u32 = 75;
const DIRECT_STEP_ERROR_PTR_LOCAL: u32 = 76;
const DIRECT_STEP_ERROR_LEN_LOCAL: u32 = 77;
const DIRECT_WHILE_PARENT_STEPS_PTR_LOCAL: u32 = 78;
const DIRECT_WHILE_PARENT_STEPS_LEN_LOCAL: u32 = 79;
/// Wall-clock deadline (ms since epoch) for an active `While` step timeout.
/// i64 local, saved/restored with the While frame so nested loops do not clobber
/// an outer loop's deadline.
const DIRECT_WHILE_DEADLINE_MS_LOCAL: u32 = 80;
/// Wall-clock deadline (ms since epoch) for an active `Split` step timeout.
/// i64 local, saved/restored with the Split frame so nested splits do not clobber
/// an outer split's deadline.
const DIRECT_SPLIT_DEADLINE_MS_LOCAL: u32 = 81;
/// Parent steps-context pointer/length saved on entry to a `Split` with an
/// onError route, so the handler runs against the parent steps (plus `__error`)
/// rather than the per-item iteration context. i32 locals.
const DIRECT_SPLIT_PARENT_STEPS_PTR_LOCAL: u32 = 82;
const DIRECT_SPLIT_PARENT_STEPS_LEN_LOCAL: u32 = 83;

/// AiAgent tool-loop scratch locals (all i32). The loop drives the `chat-turn`
/// capability: `BASE` is the constant turn config (from the input mapping),
/// `STATE` is the prior turn output carried forward, `PENDING` is this round's
/// dispatched tool results, `TURN_INPUT`/`TURN_OUT` are the current turn's
/// I/O, and the `TOOL_*` locals drive the inner tool-dispatch loop.
const DIRECT_AI_BASE_PTR_LOCAL: u32 = 84;
const DIRECT_AI_BASE_LEN_LOCAL: u32 = 85;
const DIRECT_AI_STATE_PTR_LOCAL: u32 = 86;
const DIRECT_AI_STATE_LEN_LOCAL: u32 = 87;
const DIRECT_AI_PENDING_PTR_LOCAL: u32 = 88;
const DIRECT_AI_PENDING_LEN_LOCAL: u32 = 89;
const DIRECT_AI_TURN_OUT_PTR_LOCAL: u32 = 90;
const DIRECT_AI_TURN_OUT_LEN_LOCAL: u32 = 91;
const DIRECT_AI_TURN_INPUT_PTR_LOCAL: u32 = 92;
const DIRECT_AI_TURN_INPUT_LEN_LOCAL: u32 = 93;
const DIRECT_AI_TOOL_COUNT_LOCAL: u32 = 94;
const DIRECT_AI_TOOL_IDX_LOCAL: u32 = 95;
const DIRECT_AI_TOOL_ARGS_PTR_LOCAL: u32 = 96;
const DIRECT_AI_TOOL_ARGS_LEN_LOCAL: u32 = 97;
const DIRECT_AI_TOOL_RESULT_PTR_LOCAL: u32 = 98;
const DIRECT_AI_TOOL_RESULT_LEN_LOCAL: u32 = 99;
/// Turn counter, a hard safety bound on the AiAgent tool loop.
const DIRECT_AI_ITER_LOCAL: u32 = 100;
/// The capability-resolved tool index for the current tool call (dispatch key).
const DIRECT_AI_TOOL_MATCH_LOCAL: u32 = 101;
/// The resolved conversation object (`{conversation_id}`) for memory load/save,
/// computed once before the loop and reused for the save after it.
const DIRECT_AI_CONV_PTR_LOCAL: u32 = 102;
const DIRECT_AI_CONV_LEN_LOCAL: u32 = 103;

/// The data-context pointer/length an `EmbedWorkflow` step was entered with,
/// saved before the input mapping overwrites the shared child-data local and
/// restored after the child runs. A nested embed's data context IS the child
/// -data local, so without this save its own input mapping clobbers the data the
/// step's onError handler (and following steps) reference via `data.*`. i32
/// locals, saved/restored with the embed frame so nesting stays isolated.
const DIRECT_EMBED_SAVED_DATA_PTR_LOCAL: u32 = 104;
const DIRECT_EMBED_SAVED_DATA_LEN_LOCAL: u32 = 105;

/// Monotonic per-tool-call counter for the AiAgent loop, incremented once per
/// dispatched tool call across all turns. WaitForSignal-as-tool folds it into the
/// per-call signal id so repeated calls to the same wait tool get distinct,
/// resume-stable ids (mirrors the generated `__tool_call_counter`). i32 local.
const DIRECT_AI_TOOL_CALL_COUNTER_LOCAL: u32 = 106;

/// Scratch i32 holding a Conditional step's evaluated boolean. The condition
/// result is read out of the shared retptr scratch *before* the step's
/// debug-end event (which reuses that same scratch) and stashed here, so the
/// branch decision survives the event. See the Conditional arm in
/// `compile/dispatcher.rs`. i32 local.
const DIRECT_CONDITION_RESULT_LOCAL: u32 = 107;

/// Heap watermark for a `Split`/`While` loop: the bump-allocator pointer captured
/// once the loop's surviving buffer (Split results / While state) is in place,
/// just above it. Each iteration's host-call return buffers (item, iteration
/// variables, rebuilt source, per-step outputs) are bump-allocated above this
/// mark and never freed by the core module's allocator, so without reclamation a
/// large-scope loop exhausts guest memory. At the top of each iteration the loop
/// compacts its surviving buffer down to this mark and rewinds the bump pointer,
/// bounding heap to `parent + survivor + one iteration`. Saved/restored with the
/// Split and While frames so nested loops keep distinct marks; i32 local.
const DIRECT_SPLIT_HEAP_BASE_LOCAL: u32 = 108;
const DIRECT_WHILE_HEAP_BASE_LOCAL: u32 = DIRECT_SPLIT_HEAP_BASE_LOCAL;

/// Heap watermark for the `AiAgent` chat-turn loop, the analog of
/// [`DIRECT_SPLIT_HEAP_BASE_LOCAL`] for that loop. Captured once above the
/// pre-loop persistent buffers (base turn config, conversation, initial
/// state/pending); at the top of each turn the loop bundles its cross-turn
/// survivors (state + pending) into one snapshot, compacts it back to this mark,
/// and rewinds — reclaiming the turn's model-output / tool-result scratch so a
/// long conversation does not grow guest memory per turn. A distinct local from
/// the Split/While mark so an AiAgent loop nested in a Split/While keeps its own.
/// i32 local.
const DIRECT_AI_HEAP_BASE_LOCAL: u32 = 109;

/// Per-attempt durable-retry scratch for the durable Agent retry loop. Makes each
/// failed attempt's invoke result durable so a resumed retry loop short-circuits
/// attempts that already ran (keyed `{cache_key}::attempt::{N}`) instead of
/// re-invoking the agent — see `emit_agent_plan`. All i32.
/// - `HIT_FLAG`: this attempt's result was served from a per-attempt checkpoint
///   (replay), so its backoff sleep + audit record are skipped.
/// - `ERR_FLAG`: this attempt failed (invoke error or a replayed failure); drives
///   the retry state machine in place of the raw invoke result tag.
/// - `KEY_PTR`/`KEY_LEN`: the per-attempt result key.
/// - `ENV_PTR`/`ENV_LEN`: the encoded per-attempt result envelope.
const DIRECT_AGENT_ATTEMPT_HIT_FLAG_LOCAL: u32 = 110;
const DIRECT_AGENT_ATTEMPT_ERR_FLAG_LOCAL: u32 = 111;
const DIRECT_AGENT_ATTEMPT_KEY_PTR_LOCAL: u32 = 112;
const DIRECT_AGENT_ATTEMPT_KEY_LEN_LOCAL: u32 = 113;
const DIRECT_AGENT_ATTEMPT_ENV_PTR_LOCAL: u32 = 114;
const DIRECT_AGENT_ATTEMPT_ENV_LEN_LOCAL: u32 = 115;

/// Input for the opt-in direct compiler.
#[derive(Debug, Clone)]
pub struct DirectCompilationInput {
    /// Unique workflow identifier.
    pub workflow_id: String,
    /// Workflow version number.
    pub version: u32,
    /// Optional checksum of the original workflow DSL source.
    ///
    /// Callers that still have the raw source should pass the same checksum
    /// used by the workflow image cache. `None` keeps the opt-in direct API
    /// usable in tests and internal callers that only have an `ExecutionGraph`.
    pub source_checksum: Option<String>,
    /// Parsed workflow execution graph.
    pub execution_graph: ExecutionGraph,
    /// Pre-loaded child workflows used by future static `EmbedWorkflow`
    /// lowering. Direct mode keeps this closure explicit so child graphs can be
    /// inlined into the emitted workflow-logic component instead of linked
    /// dynamically at runtime.
    pub child_workflows: Vec<crate::compile::ChildWorkflowInput>,
    /// Directory where the direct artifact directory should be created.
    pub output_dir: PathBuf,
    /// Whether to emit generated-code-compatible step debug events.
    pub track_events: bool,
    /// Runtime agent metadata catalog used to serialize capability validation.
    ///
    /// `None` falls back to the statically linked registry, matching the Rust
    /// codegen compiler's transition behavior.
    pub agent_catalog: Option<std::sync::Arc<runtara_dsl::agent_meta::AgentCatalog>>,
}

/// Result of opt-in direct workflow compilation.
#[derive(Debug, Clone)]
pub struct DirectCompilationResult {
    /// Path to the primary emitted Wasm artifact.
    ///
    /// Before static composition this is the directly emitted
    /// `workflow-logic.wasm`; after [`compose_direct_workflow`] it is the final
    /// runnable `workflow.wasm`.
    pub wasm_path: PathBuf,
    /// Path to the directly emitted workflow-logic component.
    pub workflow_logic_wasm_path: PathBuf,
    /// Path to the emitted manifest sidecar.
    pub manifest_path: PathBuf,
    /// Path to the emitted support-report sidecar.
    pub support_report_path: PathBuf,
    /// Path to the emitted artifact dependency/provenance metadata sidecar.
    pub artifact_metadata_path: PathBuf,
    /// Path to the generated component world WIT.
    pub world_wit_path: PathBuf,
    /// Path to the generated static composition script.
    pub wac_path: PathBuf,
    /// Path to the per-workflow direct build directory.
    pub build_dir: PathBuf,
    /// Size of the primary emitted Wasm artifact in bytes.
    pub wasm_size: usize,
    /// SHA-256 checksum of the primary emitted Wasm artifact.
    pub wasm_checksum: String,
    /// Size of the workflow-logic component in bytes.
    pub workflow_logic_wasm_size: usize,
    /// SHA-256 checksum of the workflow-logic component.
    pub workflow_logic_wasm_checksum: String,
    /// Path to the final statically composed `workflow.wasm`, when composed.
    pub composed_wasm_path: Option<PathBuf>,
    /// Size of the final statically composed artifact in bytes.
    pub composed_wasm_size: Option<usize>,
    /// SHA-256 checksum of the final statically composed artifact.
    pub composed_wasm_checksum: Option<String>,
    /// SHA-256 checksum embedded in the manifest.
    pub manifest_checksum: String,
    /// Deterministic support report produced before emission.
    pub support_report: DirectWorkflowSupportReport,
    /// Component-facing scaffolding emitted beside the direct artifact.
    pub component_artifacts: DirectComponentArtifacts,
    /// Dependency/provenance metadata emitted beside the direct artifact.
    pub artifact_metadata: DirectArtifactMetadata,
}

/// Compose a direct workflow logic component with prebuilt shared components.
///
/// `compile_direct_workflow` intentionally stops after direct workflow logic
/// emission. This explicit step performs the static composition that will
/// eventually produce the runnable `workflow.wasm` artifact for direct mode.
pub fn compose_direct_workflow(
    result: &mut DirectCompilationResult,
    components_dir: impl AsRef<Path>,
) -> Result<PathBuf, DirectCompileError> {
    let components_dir = components_dir.as_ref();
    let composed_path = result.build_dir.join("workflow.wasm");
    let resolve_deps_start = Instant::now();
    let shared_components = resolve_shared_component_dependencies(
        components_dir,
        &result.component_artifacts.shared_components,
    )?;
    let agent_components = resolve_agent_component_dependencies(
        components_dir,
        &result.component_artifacts.agent_components,
    )?;
    tracing::debug!(
        target: "runtara::direct_compile::profile",
        elapsed_ms = resolve_deps_start.elapsed().as_secs_f64() * 1000.0,
        shared = shared_components.len(),
        agents = agent_components.len(),
        "compose: resolved + read component dependencies from disk",
    );

    let mut overrides: HashMap<String, PathBuf> = HashMap::new();
    overrides.insert(
        "runtara:workflow-logic".to_string(),
        result.workflow_logic_wasm_path.clone(),
    );
    for component in &shared_components {
        overrides.insert(component.package.clone(), component.wasm_path.clone());
    }
    for component in &agent_components {
        overrides.insert(component.package.clone(), component.wasm_path.clone());
    }

    let compose_start = Instant::now();
    let composed_wasm = compose_workflow_component_in_process(
        &result.component_artifacts.wac_source,
        &result.build_dir,
        overrides,
    )?;
    tracing::debug!(
        target: "runtara::direct_compile::profile",
        elapsed_ms = compose_start.elapsed().as_secs_f64() * 1000.0,
        composed_bytes = composed_wasm.len(),
        "compose: in-process wac-graph composition complete",
    );
    fs::write(&composed_path, &composed_wasm)?;

    let composed_wasm_size = composed_wasm.len();
    let composed_wasm_checksum = sha256_hex(&composed_wasm);

    result.wasm_path = composed_path.clone();
    result.wasm_size = composed_wasm_size;
    result.wasm_checksum = composed_wasm_checksum.clone();
    result.composed_wasm_path = Some(composed_path.clone());
    result.composed_wasm_size = Some(composed_wasm_size);
    result.composed_wasm_checksum = Some(composed_wasm_checksum);
    result.artifact_metadata.composed_wasm = Some(DirectArtifactFileMetadata {
        filename: "workflow.wasm".to_string(),
        sha256: result.wasm_checksum.clone(),
        size_bytes: result.wasm_size as u64,
    });
    result.artifact_metadata.shared_components = shared_components
        .into_iter()
        .map(|component| component.metadata)
        .collect();
    result.artifact_metadata.agent_components = agent_components
        .into_iter()
        .map(|component| component.metadata)
        .collect();
    write_artifact_metadata(&result.artifact_metadata_path, &result.artifact_metadata)?;

    Ok(composed_path)
}

/// Statically compose a direct workflow component in-process.
///
/// This is the in-process equivalent of `wac compose <wac> -d pkg=path ...`,
/// reusing the exact `workflow.wac` document the direct compiler already emits.
/// `wac_source` is the WAC script, `deps_dir` is the search root for any package
/// not in `overrides` (none are, in practice), and `overrides` maps each WAC
/// package name (e.g. `runtara:workflow-logic`, `runtara:agent-http`) to its
/// prebuilt `.wasm` path. Composition runs through `wac-parser`/`wac-resolver`/
/// `wac-graph` — the same crates the `wac` CLI uses — so the output is identical
/// to the former subprocess, with no external `wac` binary required.
fn compose_workflow_component_in_process(
    wac_source: &str,
    deps_dir: &Path,
    overrides: HashMap<String, PathBuf>,
) -> Result<Vec<u8>, DirectCompileError> {
    use wac_graph::EncodeOptions;
    use wac_parser::Document;
    use wac_resolver::{FileSystemPackageResolver, packages};

    let parse_start = Instant::now();
    let document = Document::parse(wac_source).map_err(|err| {
        DirectCompileError::Component(format!(
            "failed to parse direct workflow wac document: {err}"
        ))
    })?;
    let keys = packages(&document).map_err(|err| {
        DirectCompileError::Component(format!(
            "failed to collect direct workflow wac packages: {err}"
        ))
    })?;
    tracing::debug!(
        target: "runtara::direct_compile::profile",
        elapsed_ms = parse_start.elapsed().as_secs_f64() * 1000.0,
        packages = keys.len(),
        "compose: parsed wac document + collected packages",
    );

    let resolve_pkgs_start = Instant::now();
    let resolver = FileSystemPackageResolver::new(deps_dir, overrides, false);
    let resolved = resolver.resolve(&keys).map_err(|err| {
        DirectCompileError::Component(format!(
            "failed to resolve direct workflow wac packages: {err}"
        ))
    })?;
    tracing::debug!(
        target: "runtara::direct_compile::profile",
        elapsed_ms = resolve_pkgs_start.elapsed().as_secs_f64() * 1000.0,
        "compose: resolved wac package bytes",
    );

    let doc_resolve_start = Instant::now();
    let resolution = document.resolve(resolved).map_err(|err| {
        DirectCompileError::Component(format!(
            "failed to resolve direct workflow wac document: {err}"
        ))
    })?;
    tracing::debug!(
        target: "runtara::direct_compile::profile",
        elapsed_ms = doc_resolve_start.elapsed().as_secs_f64() * 1000.0,
        "compose: type-checked + resolved wac document graph",
    );

    let encode_start = Instant::now();
    let encoded = resolution
        .encode(EncodeOptions {
            define_components: true,
            validate: true,
            ..Default::default()
        })
        .map_err(|err| {
            DirectCompileError::Component(format!(
                "failed to encode composed direct workflow component: {err}"
            ))
        });
    tracing::debug!(
        target: "runtara::direct_compile::profile",
        elapsed_ms = encode_start.elapsed().as_secs_f64() * 1000.0,
        validate = true,
        "compose: encoded + validated composed component",
    );
    encoded
}

/// Compile and statically compose a direct workflow into the final
/// `workflow.wasm` artifact shape used by the runtime.
pub fn compile_direct_workflow_composed(
    input: DirectCompilationInput,
    components_dir: impl AsRef<Path>,
) -> Result<DirectCompilationResult, DirectCompileError> {
    let mut result = compile_direct_workflow(input)?;
    compose_direct_workflow(&mut result, components_dir)?;
    Ok(result)
}

/// Runtime binding for production compiles, from
/// `RUNTARA_DIRECT_RUNTIME_BINDING` — the operational rollback lever.
///
/// Default (unset or anything else): `HostImport`. `composed` reverts new
/// compiles to the legacy composed-runtime shape (guest HTTP loopback), which
/// the runner still executes fully — set it on the server and recompile if a
/// host-import regression ever needs a same-day escape hatch.
fn runtime_binding_from_env() -> super::component::RuntimeBinding {
    runtime_binding_from_raw(
        std::env::var("RUNTARA_DIRECT_RUNTIME_BINDING")
            .ok()
            .as_deref(),
    )
}

fn runtime_binding_from_raw(raw: Option<&str>) -> super::component::RuntimeBinding {
    match raw {
        Some("composed") => super::component::RuntimeBinding::Composed,
        _ => super::component::RuntimeBinding::HostImport,
    }
}

/// [`compile_direct_workflow_composed`] with an explicit [`RuntimeBinding`],
/// re-emitting the component scaffolding under `binding` before composing.
///
/// Used where the default (HostImport) binding cannot run: the wasmtime-CLI
/// A/B reference axis has no way to satisfy host imports, so it composes the
/// legacy runtime component in — and by binding-differential tests comparing
/// the two artifact shapes.
pub fn compile_direct_workflow_composed_with_binding(
    input: DirectCompilationInput,
    components_dir: impl AsRef<Path>,
    binding: super::component::RuntimeBinding,
) -> Result<DirectCompilationResult, DirectCompileError> {
    compile_direct_workflow_composed_configured(
        input,
        components_dir,
        binding,
        super::component::WorkflowAbi::default(),
    )
}

/// Fully-configured compile+compose: explicit [`RuntimeBinding`] AND
/// [`super::component::WorkflowAbi`] — the entry the ABI-differential test
/// axis drives.
pub fn compile_direct_workflow_composed_configured(
    input: DirectCompilationInput,
    components_dir: impl AsRef<Path>,
    binding: super::component::RuntimeBinding,
    abi: super::component::WorkflowAbi,
) -> Result<DirectCompilationResult, DirectCompileError> {
    let mut result = compile_direct_workflow_with_abi(input, abi)?;
    let agent_ids: Vec<String> = result
        .component_artifacts
        .agent_components
        .iter()
        .map(|component| component.agent_id.clone())
        .collect();
    result.component_artifacts =
        super::component::emit_direct_component_artifacts_configured(&agent_ids, binding, abi);
    // Keep the on-disk scaffolding consistent with what is composed.
    fs::write(
        &result.world_wit_path,
        &result.component_artifacts.world_wit,
    )?;
    fs::write(&result.wac_path, &result.component_artifacts.wac_source)?;
    compose_direct_workflow(&mut result, components_dir)?;
    Ok(result)
}

/// Stack size for the dedicated compile thread.
///
/// Run-plan construction, Wasm emission, and the run-plan drop all recurse
/// proportionally to the longest unconditional step chain in the graph. On the
/// 2 MiB default stack of `tokio::task::spawn_blocking` threads a release
/// build overflows — aborting the whole process — between 400 and 800 chained
/// steps, and a debug build around 100. This is address space, not committed
/// memory: pages are only touched as the recursion actually deepens, and it
/// buys two orders of magnitude of headroom over the largest graphs seen in
/// practice.
const DIRECT_COMPILE_STACK_SIZE: usize = 256 * 1024 * 1024;

/// Compile a workflow through the direct path — the only compile path.
///
/// Accepts exactly the graphs passed by [`super::support::analyze_direct_wasm_support`];
/// anything else is a hard [`DirectCompileError::Unsupported`] carrying the
/// per-feature report. The emitted component-format artifact is a stable
/// direct pipeline artifact with a canonical `wasi:cli/run` export, stdlib
/// JSON calls, and runtime completion calls.
///
/// Runs on a dedicated thread with an explicit [`DIRECT_COMPILE_STACK_SIZE`]
/// stack so compilation never depends on the caller's stack budget; panics
/// from the compile body resume on the caller.
pub fn compile_direct_workflow(
    input: DirectCompilationInput,
) -> Result<DirectCompilationResult, DirectCompileError> {
    compile_direct_workflow_with_abi(input, super::component::WorkflowAbi::default())
}

/// [`compile_direct_workflow`] with an explicit [`super::component::WorkflowAbi`]
/// — the flag-gated invoke-export path (Phase 3 of
/// docs/unify-agents-workflows-plan.md). The default entry keeps the legacy
/// `wasi:cli/run` shape untouched.
pub fn compile_direct_workflow_with_abi(
    input: DirectCompilationInput,
    abi: super::component::WorkflowAbi,
) -> Result<DirectCompilationResult, DirectCompileError> {
    let span = tracing::Span::current();
    let handle = std::thread::Builder::new()
        .name("direct-compile".to_string())
        .stack_size(DIRECT_COMPILE_STACK_SIZE)
        .spawn(move || {
            let _span = span.entered();
            compile_direct_workflow_inner(input, abi)
        })
        .map_err(DirectCompileError::Io)?;
    match handle.join() {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

fn compile_direct_workflow_inner(
    input: DirectCompilationInput,
    abi: super::component::WorkflowAbi,
) -> Result<DirectCompilationResult, DirectCompileError> {
    // The agent catalog is supplied by the caller (the server passes the
    // runtime catalog loaded from component `meta.json`). When absent, the
    // manifest builder handles `None` directly — there is no static-registry
    // fallback.
    let agent_catalog = input.agent_catalog.as_deref();
    let child_manifest_inputs = input
        .child_workflows
        .iter()
        .map(|child| DirectManifestChildWorkflowInput {
            step_id: child.step_id.as_str(),
            workflow_id: child.workflow_id.as_str(),
            version_requested: child.version_requested.as_str(),
            version_resolved: child.version_resolved,
            execution_graph: &child.execution_graph,
        })
        .collect::<Vec<_>>();
    let manifest = build_direct_workflow_manifest_with_child_workflows_and_agent_catalog(
        &input.execution_graph,
        &child_manifest_inputs,
        agent_catalog,
    )?;
    let support_report = analyze_direct_wasm_support_with_child_workflows(
        &input.execution_graph,
        &input.child_workflows,
    );
    if !support_report.supported {
        return Err(DirectCompileError::Unsupported {
            report: Box::new(support_report),
        });
    }
    let child_workflow_metadata =
        resolve_direct_child_workflow_metadata(&manifest, &input.child_workflows)?;

    let manifest_json = manifest.to_canonical_json()?;
    let support_json = serde_json::to_vec(&support_report)?;
    let wasm = emit_direct_artifact(
        &manifest,
        &manifest_json,
        &support_json,
        input.track_events,
        &input.workflow_id,
        abi,
    )?;
    let wasm_checksum = sha256_hex(&wasm);
    let support_report_checksum = sha256_hex(&support_json);
    let component_artifacts = super::component::emit_direct_component_artifacts_configured(
        &manifest.feature_summary.agent_ids,
        runtime_binding_from_env(),
        abi,
    );

    let build_dir = input.output_dir.join(format!(
        "{}-v{}-direct",
        sanitize_path_segment(&input.workflow_id),
        input.version
    ));
    fs::create_dir_all(&build_dir)?;
    fs::create_dir_all(build_dir.join("wit"))?;

    let wasm_path = build_dir.join("workflow-logic.wasm");
    let manifest_path = build_dir.join("manifest.json");
    let support_report_path = build_dir.join("support-report.json");
    let artifact_metadata_path = build_dir.join(DIRECT_WORKFLOW_ARTIFACT_METADATA_FILENAME);
    let world_wit_path = build_dir.join("wit/world.wit");
    let wac_path = build_dir.join("workflow.wac");
    let artifact_metadata = initial_artifact_metadata(InitialArtifactMetadataInput {
        workflow_id: &input.workflow_id,
        workflow_version: input.version,
        source_checksum: input.source_checksum.as_deref(),
        manifest_checksum: manifest.checksum(),
        support_report_checksum: &support_report_checksum,
        workflow_logic_checksum: &wasm_checksum,
        workflow_logic_size: wasm.len(),
        component_artifacts: &component_artifacts,
        child_workflows: &child_workflow_metadata,
    });

    fs::write(&wasm_path, &wasm)?;
    fs::write(&manifest_path, &manifest_json)?;
    fs::write(&support_report_path, &support_json)?;
    write_artifact_metadata(&artifact_metadata_path, &artifact_metadata)?;
    fs::write(&world_wit_path, &component_artifacts.world_wit)?;
    fs::write(&wac_path, &component_artifacts.wac_source)?;

    Ok(DirectCompilationResult {
        wasm_path,
        workflow_logic_wasm_path: build_dir.join("workflow-logic.wasm"),
        manifest_path,
        support_report_path,
        artifact_metadata_path,
        world_wit_path,
        wac_path,
        build_dir,
        wasm_size: wasm.len(),
        wasm_checksum: wasm_checksum.clone(),
        workflow_logic_wasm_size: wasm.len(),
        workflow_logic_wasm_checksum: wasm_checksum,
        composed_wasm_path: None,
        composed_wasm_size: None,
        composed_wasm_checksum: None,
        manifest_checksum: manifest.checksum().to_string(),
        support_report,
        component_artifacts,
        artifact_metadata,
    })
}

fn emit_direct_artifact(
    manifest: &DirectWorkflowManifest,
    manifest_json: &[u8],
    support_json: &[u8],
    track_events: bool,
    workflow_id: &str,
    abi: super::component::WorkflowAbi,
) -> Result<Vec<u8>, DirectCompileError> {
    let abi_json = match abi {
        super::component::WorkflowAbi::CliRunHttp => serde_json::to_vec(&serde_json::json!({
            "abiVersion": DIRECT_WORKFLOW_ABI_VERSION,
            "artifactKind": "direct-run-component",
            "componentRunExport": "wasi:cli/run@0.2.3",
            "entryPointExecutable": true,
            "runtimeExecutable": true,
            "outputMode": "stdlib-apply-mapping",
            "manifestVersion": DIRECT_WORKFLOW_MANIFEST_VERSION,
            "stepCount": manifest.feature_summary.total_steps,
            "note": "direct compiler component with canonical run export, stdlib mapping/condition calls, and runtime.complete call"
        }))?,
        super::component::WorkflowAbi::InvokeHostImports => {
            serde_json::to_vec(&serde_json::json!({
                "abiVersion": DIRECT_WORKFLOW_INVOKE_ABI_VERSION,
                "artifactKind": "direct-invoke-component",
                "componentRunExport": LIFECYCLE_INTERFACE_NAME,
                "entryPointExecutable": true,
                "runtimeExecutable": true,
                "outputMode": "invoke-result-outcome",
                "manifestVersion": DIRECT_WORKFLOW_MANIFEST_VERSION,
                "stepCount": manifest.feature_summary.total_steps,
                "note": "unified invoke export: input as the call argument, terminal result as result<outcome, error-info>; runtime interface host-satisfied; complete/fail still fire additively"
            }))?
        }
    };

    let mut component =
        emit_direct_component(manifest, manifest_json, track_events, workflow_id, abi)?;
    append_component_custom_section(&mut component, DIRECT_WORKFLOW_ABI_SECTION, &abi_json);
    append_component_custom_section(
        &mut component,
        DIRECT_WORKFLOW_MANIFEST_SECTION,
        manifest_json,
    );
    append_component_custom_section(
        &mut component,
        DIRECT_WORKFLOW_SUPPORT_SECTION,
        support_json,
    );

    Ok(component)
}

fn emit_direct_component(
    manifest: &DirectWorkflowManifest,
    manifest_json: &[u8],
    track_events: bool,
    workflow_id: &str,
    abi: super::component::WorkflowAbi,
) -> Result<Vec<u8>, DirectCompileError> {
    let (resolve, world) =
        build_direct_component_resolve_configured(&manifest.feature_summary.agent_ids, abi)?;
    let core_config =
        DirectCoreConfig::new_with_workflow_id(manifest, manifest_json, track_events, workflow_id)?
            .with_abi(abi);
    let mut core_module = emit_direct_core_module(&resolve, world, &core_config)?;
    embed_component_metadata(&mut core_module, &resolve, world, StringEncoding::UTF8)
        .map_err(component_error)?;

    ComponentEncoder::default()
        .module(&core_module)
        .map_err(component_error)?
        .validate(true)
        .encode()
        .map_err(component_error)
}

#[cfg(test)]
fn build_direct_component_resolve() -> Result<(Resolve, WorldId), DirectCompileError> {
    build_direct_component_resolve_configured(&[], super::component::WorkflowAbi::default())
}

#[cfg(test)]
fn build_direct_component_resolve_with_agents(
    agents: &[String],
) -> Result<(Resolve, WorldId), DirectCompileError> {
    build_direct_component_resolve_configured(agents, super::component::WorkflowAbi::default())
}

fn build_direct_component_resolve_configured(
    agents: &[String],
    abi: super::component::WorkflowAbi,
) -> Result<(Resolve, WorldId), DirectCompileError> {
    let mut resolve = Resolve::default();
    resolve
        .push_str("runtara-workflow-stdlib.wit", STDLIB_WIT)
        .map_err(component_error)?;
    resolve
        .push_str("runtara-workflow-runtime.wit", RUNTIME_WIT)
        .map_err(component_error)?;
    match abi {
        super::component::WorkflowAbi::CliRunHttp => {
            resolve
                .push_str("wasi-cli-run.wit", WASI_CLI_RUN_WIT)
                .map_err(component_error)?;
        }
        super::component::WorkflowAbi::InvokeHostImports => {
            resolve
                .push_str("runtara-workflow-lifecycle.wit", LIFECYCLE_WIT)
                .map_err(component_error)?;
        }
    }
    if !agents.is_empty() {
        resolve
            .push_str("runtara-agent-types.wit", AGENT_TYPES_WIT)
            .map_err(component_error)?;
        for agent in agents {
            resolve
                .push_str(
                    format!("runtara-agent-{agent}.wit"),
                    &agent_wit_package(agent),
                )
                .map_err(component_error)?;
        }
    }

    let mut workflow_wit = format!(
        "package runtara:workflow@{WORKFLOW_WIT_VERSION};\n\
         \n\
         world workflow {{\n\
             import runtara:workflow-stdlib/json@{WORKFLOW_WIT_VERSION};\n\
             import runtara:workflow-runtime/runtime@{WORKFLOW_WIT_VERSION};\n"
    );
    for agent in agents {
        workflow_wit.push_str(&format!(
            "    import runtara:agent-{agent}/capabilities@{AGENT_WIT_VERSION};\n",
        ));
    }
    match abi {
        super::component::WorkflowAbi::CliRunHttp => {
            workflow_wit.push_str("    export wasi:cli/run@0.2.3;\n")
        }
        super::component::WorkflowAbi::InvokeHostImports => {
            workflow_wit.push_str(&format!("    export {LIFECYCLE_INTERFACE_NAME};\n"))
        }
    }
    workflow_wit.push_str("}\n");
    let package = resolve
        .push_str("runtara-workflow.wit", &workflow_wit)
        .map_err(component_error)?;
    let world = resolve
        .select_world(&[package], Some("workflow"))
        .map_err(component_error)?;

    Ok((resolve, world))
}

fn agent_wit_package(agent: &str) -> String {
    format!(
        "package runtara:agent-{agent}@{AGENT_WIT_VERSION};\n\
         \n\
         interface capabilities {{\n\
             use runtara:agent/types@{AGENT_WIT_VERSION}.{{connection-info, error-info}};\n\
             invoke: func(\n\
                 capability-id: string,\n\
                 input: list<u8>,\n\
                 connection: option<connection-info>,\n\
             ) -> result<list<u8>, error-info>;\n\
         }}\n\
         \n\
         world agent {{\n\
             export capabilities;\n\
         }}\n"
    )
}

/// Terminal-failure return — the ONE place that owns the per-ABI exit shape.
/// Every fail site in every lowerer funnels here (directly or via the
/// `abi.rs` retptr-error wrappers).
///
/// Both ABIs still record the failure host-side via `runtime.fail` (additive
/// during the migration). The return differs:
/// - `wasi:cli/run`: the classic non-zero result tag.
/// - invoke export: `Err(error-info)` written into the fixed result area at
///   offset 0 (the low retptr scratch — dead here by construction: no host
///   call runs between this write and the canonical lift, and it is
///   8-aligned as the payload requires). Layout (payload @8, align 8):
///   code@8/12, message@16/20, category@24/28, severity@32/36, retryable@40,
///   retry-after-ms tag@48 val@56, attributes tag@64 str@68/72 — total 80.
///   v1 wraps the raw error bytes as `message` and leaves code/category/
///   severity as empty strings (zeroed ptr/len is a valid empty string);
///   structured mapping arrives with the suspend wiring phase.
fn emit_runtime_fail_return(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    error_ptr_local: u32,
    error_len_local: u32,
) {
    body.instruction(&Instruction::LocalGet(error_ptr_local));
    body.instruction(&Instruction::LocalGet(error_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_fail));
    match indices.abi {
        super::component::WorkflowAbi::CliRunHttp => {
            body.instruction(&Instruction::I32Const(1));
            body.instruction(&Instruction::Return);
        }
        super::component::WorkflowAbi::InvokeHostImports => {
            body.instruction(&Instruction::I32Const(0));
            body.instruction(&Instruction::I32Const(0));
            body.instruction(&Instruction::I32Const(80));
            body.instruction(&Instruction::MemoryFill(0));
            // result disc = 1 (err)
            body.instruction(&Instruction::I32Const(0));
            body.instruction(&Instruction::I32Const(1));
            body.instruction(&Instruction::I32Store8(wasm_encoder::MemArg {
                offset: 0,
                align: 0,
                memory_index: 0,
            }));
            // error-info.message = the raw error bytes
            body.instruction(&Instruction::I32Const(0));
            body.instruction(&Instruction::LocalGet(error_ptr_local));
            body.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                offset: 16,
                align: 2,
                memory_index: 0,
            }));
            body.instruction(&Instruction::I32Const(0));
            body.instruction(&Instruction::LocalGet(error_len_local));
            body.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                offset: 20,
                align: 2,
                memory_index: 0,
            }));
            body.instruction(&Instruction::I32Const(0));
            body.instruction(&Instruction::Return);
        }
    }
}

fn append_component_custom_section(bytes: &mut Vec<u8>, name: &str, data: &[u8]) {
    let section = CustomSection {
        name: Cow::Borrowed(name),
        data: Cow::Borrowed(data),
    };
    bytes.push(section.id());
    section.encode(bytes);
}

fn component_error(error: impl fmt::Display) -> DirectCompileError {
    DirectCompileError::Component(error.to_string())
}

fn sanitize_path_segment(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();

    let trimmed = sanitized.trim_matches('-');
    if trimmed.is_empty() {
        "workflow".to_string()
    } else {
        trimmed.to_string()
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests;
