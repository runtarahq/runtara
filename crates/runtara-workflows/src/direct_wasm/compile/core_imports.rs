// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Direct core WIT import indexing and import/export classifiers.
//!
//! Core Wasm calls functions by numeric index in declaration order, but the
//! emitter wants to call them by meaning ("apply-mapping", "load-input"). This is
//! the name-to-index binding layer: as the world's imports are walked,
//! `import_core_function` declares each host/stdlib/per-agent WIT function and
//! records its assigned index into `DirectCoreImportIndices`; `require_all` then
//! converts that into the non-optional `DirectCoreFunctionIndices`, turning a
//! missing import (e.g. from a stale WIT world) into a hard compile error rather
//! than a module that traps at link time.

use std::collections::BTreeMap;

use wasm_encoder::{EntityType, ImportSection, TypeSection};
use wit_parser::abi::WasmType;
use wit_parser::{Function as WitFunction, ManglingAndAbi, Resolve, WasmImport, WorldKey};

use super::DirectCompileError;
use super::abi::push_core_type;

#[derive(Debug, Default)]
pub(super) struct DirectCoreImportIndices {
    runtime_load_input: Option<u32>,
    runtime_complete: Option<u32>,
    runtime_fail: Option<u32>,
    runtime_custom_event: Option<u32>,
    runtime_debug_mode_enabled: Option<u32>,
    runtime_breakpoint_pause: Option<u32>,
    runtime_heartbeat: Option<u32>,
    runtime_instance_id: Option<u32>,
    runtime_is_cancelled: Option<u32>,
    runtime_check_signals: Option<u32>,
    runtime_poll_custom_signal: Option<u32>,
    runtime_now_ms: Option<u32>,
    runtime_get_checkpoint: Option<u32>,
    runtime_checkpoint: Option<u32>,
    runtime_handle_checkpoint_signal: Option<u32>,
    runtime_record_retry_attempt: Option<u32>,
    runtime_durable_sleep: Option<u32>,
    runtime_blocking_sleep: Option<u32>,
    runtime_durable_sleep_checkpoint: Option<u32>,
    stdlib_init_manifest: Option<u32>,
    stdlib_value_store_retain: Option<u32>,
    stdlib_build_source: Option<u32>,
    stdlib_apply_mapping: Option<u32>,
    stdlib_eval_condition: Option<u32>,
    stdlib_process_switch: Option<u32>,
    stdlib_filter: Option<u32>,
    stdlib_log_event: Option<u32>,
    stdlib_log: Option<u32>,
    stdlib_error_event: Option<u32>,
    stdlib_error: Option<u32>,
    stdlib_error_steps: Option<u32>,
    stdlib_value_switch: Option<u32>,
    stdlib_group_by: Option<u32>,
    stdlib_split_item_count: Option<u32>,
    stdlib_split_item: Option<u32>,
    stdlib_split_iteration_variables: Option<u32>,
    stdlib_split_validate_input: Option<u32>,
    stdlib_split_validate_output: Option<u32>,
    stdlib_split_initial_results: Option<u32>,
    stdlib_split_append_output: Option<u32>,
    stdlib_split_append_error: Option<u32>,
    stdlib_split_output: Option<u32>,
    stdlib_split_cache_key: Option<u32>,
    stdlib_split_result: Option<u32>,
    stdlib_split_output_from_result: Option<u32>,
    stdlib_while_max_iterations: Option<u32>,
    stdlib_while_initial_state: Option<u32>,
    stdlib_while_condition_source: Option<u32>,
    stdlib_while_condition: Option<u32>,
    stdlib_while_iteration_variables: Option<u32>,
    stdlib_while_advance_state: Option<u32>,
    stdlib_while_output: Option<u32>,
    stdlib_delay_duration_ms: Option<u32>,
    stdlib_delay: Option<u32>,
    stdlib_delay_sleep_key: Option<u32>,
    stdlib_invoke_error_fields: Option<u32>,
    stdlib_breakpoint_key: Option<u32>,
    stdlib_breakpoint_event: Option<u32>,
    stdlib_wait_signal_id: Option<u32>,
    stdlib_wait_timeout_ms: Option<u32>,
    stdlib_wait_timeout_error: Option<u32>,
    stdlib_wait_on_wait_variables: Option<u32>,
    stdlib_wait_on_wait_error: Option<u32>,
    stdlib_wait_poll_interval_ms: Option<u32>,
    stdlib_wait_event: Option<u32>,
    stdlib_wait_debug_start: Option<u32>,
    stdlib_wait_output: Option<u32>,
    stdlib_ai_wait_tool_signal_id: Option<u32>,
    stdlib_ai_wait_tool_result: Option<u32>,
    stdlib_embed_workflow_cache_key: Option<u32>,
    stdlib_embed_workflow_variables: Option<u32>,
    stdlib_embed_workflow_result: Option<u32>,
    stdlib_embed_workflow_output_from_result: Option<u32>,
    stdlib_embed_workflow_error: Option<u32>,
    stdlib_retry_sleep_key: Option<u32>,
    stdlib_retry_delay_ms: Option<u32>,
    stdlib_workflow_error_retryable: Option<u32>,
    stdlib_workflow_error_rate_limited: Option<u32>,
    stdlib_workflow_error_retry_after_ms: Option<u32>,
    stdlib_agent_output: Option<u32>,
    stdlib_ai_agent_output: Option<u32>,
    stdlib_ai_turn_next_input: Option<u32>,
    stdlib_ai_turn_is_complete: Option<u32>,
    stdlib_ai_turn_tool_count: Option<u32>,
    stdlib_ai_turn_tool_args: Option<u32>,
    stdlib_ai_turn_tool_index: Option<u32>,
    stdlib_ai_tool_args_with_timeout: Option<u32>,
    stdlib_ai_turn_add_result: Option<u32>,
    stdlib_wait_timeout_error_envelope: Option<u32>,
    stdlib_ai_turn_cache_key: Option<u32>,
    stdlib_ai_turn_snapshot: Option<u32>,
    stdlib_ai_turn_snapshot_part: Option<u32>,
    stdlib_ai_turn_snapshot_tool_calls: Option<u32>,
    stdlib_ai_turn_snapshot_complete: Option<u32>,
    stdlib_ai_turn_output: Option<u32>,
    stdlib_ai_tool_debug_start: Option<u32>,
    stdlib_ai_tool_debug_end: Option<u32>,
    stdlib_ai_memory_debug_start: Option<u32>,
    stdlib_ai_memory_debug_end: Option<u32>,
    stdlib_ai_memory_initial_state: Option<u32>,
    stdlib_ai_memory_save_input: Option<u32>,
    stdlib_ai_memory_compact_sliding: Option<u32>,
    stdlib_ai_summarize_input: Option<u32>,
    stdlib_ai_summarize_output: Option<u32>,
    stdlib_agent_validate_input: Option<u32>,
    stdlib_agent_connection_input: Option<u32>,
    stdlib_agent_scope_input: Option<u32>,
    stdlib_agent_tool_scope_input: Option<u32>,
    stdlib_agent_cache_key: Option<u32>,
    stdlib_agent_retry_sleep_key: Option<u32>,
    stdlib_agent_attempt_result_key: Option<u32>,
    stdlib_agent_attempt_envelope: Option<u32>,
    stdlib_agent_retry_delay_ms: Option<u32>,
    stdlib_agent_error_info: Option<u32>,
    stdlib_agent_retry_error_info: Option<u32>,
    stdlib_agent_error: Option<u32>,
    stdlib_agent_error_from_info: Option<u32>,
    stdlib_agent_debug_error: Option<u32>,
    stdlib_step_debug_start: Option<u32>,
    stdlib_step_debug_end: Option<u32>,
    stdlib_step_debug_error: Option<u32>,
    agent_invokes: BTreeMap<String, DirectAgentInvokeImport>,
}

impl DirectCoreImportIndices {
    pub(super) fn require_all(
        self,
        abi: crate::direct_wasm::component::WorkflowAbi,
        store_freeing_sleep: bool,
        omit_runtime: bool,
    ) -> Result<DirectCoreFunctionIndices, DirectCompileError> {
        let _stdlib_agent_error_info =
            require_import(self.stdlib_agent_error_info, "stdlib.agent-error-info")?;
        Ok(DirectCoreFunctionIndices {
            abi,
            store_freeing_sleep,
            omit_runtime,
            runtime_load_input: require_runtime(
                self.runtime_load_input,
                "runtime.load-input",
                omit_runtime,
            )?,
            runtime_complete: require_runtime(
                self.runtime_complete,
                "runtime.complete",
                omit_runtime,
            )?,
            runtime_fail: require_runtime(self.runtime_fail, "runtime.fail", omit_runtime)?,
            runtime_custom_event: require_runtime(
                self.runtime_custom_event,
                "runtime.custom-event",
                omit_runtime,
            )?,
            runtime_debug_mode_enabled: require_runtime(
                self.runtime_debug_mode_enabled,
                "runtime.debug-mode-enabled",
                omit_runtime,
            )?,
            runtime_breakpoint_pause: require_runtime(
                self.runtime_breakpoint_pause,
                "runtime.breakpoint-pause",
                omit_runtime,
            )?,
            runtime_heartbeat: require_runtime(
                self.runtime_heartbeat,
                "runtime.heartbeat",
                omit_runtime,
            )?,
            runtime_instance_id: require_runtime(
                self.runtime_instance_id,
                "runtime.instance-id",
                omit_runtime,
            )?,
            runtime_is_cancelled: require_runtime(
                self.runtime_is_cancelled,
                "runtime.is-cancelled",
                omit_runtime,
            )?,
            runtime_check_signals: require_runtime(
                self.runtime_check_signals,
                "runtime.check-signals",
                omit_runtime,
            )?,
            runtime_poll_custom_signal: require_runtime(
                self.runtime_poll_custom_signal,
                "runtime.poll-custom-signal",
                omit_runtime,
            )?,
            runtime_now_ms: require_runtime(self.runtime_now_ms, "runtime.now-ms", omit_runtime)?,
            runtime_get_checkpoint: require_runtime(
                self.runtime_get_checkpoint,
                "runtime.get-checkpoint",
                omit_runtime,
            )?,
            runtime_checkpoint: require_runtime(
                self.runtime_checkpoint,
                "runtime.checkpoint",
                omit_runtime,
            )?,
            runtime_handle_checkpoint_signal: require_runtime(
                self.runtime_handle_checkpoint_signal,
                "runtime.handle-checkpoint-signal",
                omit_runtime,
            )?,
            runtime_record_retry_attempt: require_runtime(
                self.runtime_record_retry_attempt,
                "runtime.record-retry-attempt",
                omit_runtime,
            )?,
            runtime_durable_sleep: require_runtime(
                self.runtime_durable_sleep,
                "runtime.durable-sleep",
                omit_runtime,
            )?,
            runtime_blocking_sleep: require_runtime(
                self.runtime_blocking_sleep,
                "runtime.blocking-sleep",
                omit_runtime,
            )?,
            runtime_durable_sleep_checkpoint: require_runtime(
                self.runtime_durable_sleep_checkpoint,
                "runtime.durable-sleep-checkpoint",
                omit_runtime,
            )?,
            stdlib_init_manifest: require_import(
                self.stdlib_init_manifest,
                "stdlib.init-manifest",
            )?,
            stdlib_value_store_retain: require_import(
                self.stdlib_value_store_retain,
                "stdlib.value-store-retain",
            )?,
            stdlib_build_source: require_import(self.stdlib_build_source, "stdlib.build-source")?,
            stdlib_apply_mapping: require_import(
                self.stdlib_apply_mapping,
                "stdlib.apply-mapping",
            )?,
            stdlib_eval_condition: require_import(
                self.stdlib_eval_condition,
                "stdlib.eval-condition",
            )?,
            stdlib_process_switch: require_import(
                self.stdlib_process_switch,
                "stdlib.process-switch",
            )?,
            stdlib_filter: require_import(self.stdlib_filter, "stdlib.filter")?,
            stdlib_log_event: require_import(self.stdlib_log_event, "stdlib.log-event")?,
            stdlib_log: require_import(self.stdlib_log, "stdlib.log")?,
            stdlib_error_event: require_import(self.stdlib_error_event, "stdlib.error-event")?,
            stdlib_error: require_import(self.stdlib_error, "stdlib.error")?,
            stdlib_error_steps: require_import(self.stdlib_error_steps, "stdlib.error-steps")?,
            stdlib_value_switch: require_import(self.stdlib_value_switch, "stdlib.value-switch")?,
            stdlib_group_by: require_import(self.stdlib_group_by, "stdlib.group-by")?,
            stdlib_split_item_count: require_import(
                self.stdlib_split_item_count,
                "stdlib.split-item-count",
            )?,
            stdlib_split_item: require_import(self.stdlib_split_item, "stdlib.split-item")?,
            stdlib_split_iteration_variables: require_import(
                self.stdlib_split_iteration_variables,
                "stdlib.split-iteration-variables",
            )?,
            stdlib_split_validate_input: require_import(
                self.stdlib_split_validate_input,
                "stdlib.split-validate-input",
            )?,
            stdlib_split_validate_output: require_import(
                self.stdlib_split_validate_output,
                "stdlib.split-validate-output",
            )?,
            stdlib_split_initial_results: require_import(
                self.stdlib_split_initial_results,
                "stdlib.split-initial-results",
            )?,
            stdlib_split_append_output: require_import(
                self.stdlib_split_append_output,
                "stdlib.split-append-output",
            )?,
            stdlib_split_append_error: require_import(
                self.stdlib_split_append_error,
                "stdlib.split-append-error",
            )?,
            stdlib_split_output: require_import(self.stdlib_split_output, "stdlib.split-output")?,
            stdlib_split_cache_key: require_import(
                self.stdlib_split_cache_key,
                "stdlib.split-cache-key",
            )?,
            stdlib_split_result: require_import(self.stdlib_split_result, "stdlib.split-result")?,
            stdlib_split_output_from_result: require_import(
                self.stdlib_split_output_from_result,
                "stdlib.split-output-from-result",
            )?,
            stdlib_while_max_iterations: require_import(
                self.stdlib_while_max_iterations,
                "stdlib.while-max-iterations",
            )?,
            stdlib_while_initial_state: require_import(
                self.stdlib_while_initial_state,
                "stdlib.while-initial-state",
            )?,
            stdlib_while_condition_source: require_import(
                self.stdlib_while_condition_source,
                "stdlib.while-condition-source",
            )?,
            stdlib_while_condition: require_import(
                self.stdlib_while_condition,
                "stdlib.while-condition",
            )?,
            stdlib_while_iteration_variables: require_import(
                self.stdlib_while_iteration_variables,
                "stdlib.while-iteration-variables",
            )?,
            stdlib_while_advance_state: require_import(
                self.stdlib_while_advance_state,
                "stdlib.while-advance-state",
            )?,
            stdlib_while_output: require_import(self.stdlib_while_output, "stdlib.while-output")?,
            stdlib_delay_duration_ms: require_import(
                self.stdlib_delay_duration_ms,
                "stdlib.delay-duration-ms",
            )?,
            stdlib_delay: require_import(self.stdlib_delay, "stdlib.delay")?,
            stdlib_delay_sleep_key: require_import(
                self.stdlib_delay_sleep_key,
                "stdlib.delay-sleep-key",
            )?,
            stdlib_invoke_error_fields: require_import(
                self.stdlib_invoke_error_fields,
                "stdlib.invoke-error-fields",
            )?,
            stdlib_breakpoint_key: require_import(
                self.stdlib_breakpoint_key,
                "stdlib.breakpoint-key",
            )?,
            stdlib_breakpoint_event: require_import(
                self.stdlib_breakpoint_event,
                "stdlib.breakpoint-event",
            )?,
            stdlib_wait_signal_id: require_import(
                self.stdlib_wait_signal_id,
                "stdlib.wait-signal-id",
            )?,
            stdlib_wait_timeout_ms: require_import(
                self.stdlib_wait_timeout_ms,
                "stdlib.wait-timeout-ms",
            )?,
            stdlib_wait_timeout_error: require_import(
                self.stdlib_wait_timeout_error,
                "stdlib.wait-timeout-error",
            )?,
            stdlib_wait_on_wait_variables: require_import(
                self.stdlib_wait_on_wait_variables,
                "stdlib.wait-on-wait-variables",
            )?,
            stdlib_wait_on_wait_error: require_import(
                self.stdlib_wait_on_wait_error,
                "stdlib.wait-on-wait-error",
            )?,
            stdlib_wait_poll_interval_ms: require_import(
                self.stdlib_wait_poll_interval_ms,
                "stdlib.wait-poll-interval-ms",
            )?,
            stdlib_wait_event: require_import(self.stdlib_wait_event, "stdlib.wait-event")?,
            stdlib_wait_debug_start: require_import(
                self.stdlib_wait_debug_start,
                "stdlib.wait-debug-start",
            )?,
            stdlib_wait_output: require_import(self.stdlib_wait_output, "stdlib.wait-output")?,
            stdlib_ai_wait_tool_signal_id: require_import(
                self.stdlib_ai_wait_tool_signal_id,
                "stdlib.ai-wait-tool-signal-id",
            )?,
            stdlib_ai_wait_tool_result: require_import(
                self.stdlib_ai_wait_tool_result,
                "stdlib.ai-wait-tool-result",
            )?,
            stdlib_embed_workflow_cache_key: require_import(
                self.stdlib_embed_workflow_cache_key,
                "stdlib.embed-workflow-cache-key",
            )?,
            stdlib_embed_workflow_variables: require_import(
                self.stdlib_embed_workflow_variables,
                "stdlib.embed-workflow-variables",
            )?,
            stdlib_embed_workflow_result: require_import(
                self.stdlib_embed_workflow_result,
                "stdlib.embed-workflow-result",
            )?,
            stdlib_embed_workflow_output_from_result: require_import(
                self.stdlib_embed_workflow_output_from_result,
                "stdlib.embed-workflow-output-from-result",
            )?,
            stdlib_embed_workflow_error: require_import(
                self.stdlib_embed_workflow_error,
                "stdlib.embed-workflow-error",
            )?,
            stdlib_retry_sleep_key: require_import(
                self.stdlib_retry_sleep_key,
                "stdlib.retry-sleep-key",
            )?,
            stdlib_retry_delay_ms: require_import(
                self.stdlib_retry_delay_ms,
                "stdlib.retry-delay-ms",
            )?,
            stdlib_workflow_error_retryable: require_import(
                self.stdlib_workflow_error_retryable,
                "stdlib.workflow-error-retryable",
            )?,
            stdlib_workflow_error_rate_limited: require_import(
                self.stdlib_workflow_error_rate_limited,
                "stdlib.workflow-error-rate-limited",
            )?,
            stdlib_workflow_error_retry_after_ms: require_import(
                self.stdlib_workflow_error_retry_after_ms,
                "stdlib.workflow-error-retry-after-ms",
            )?,
            stdlib_agent_output: require_import(self.stdlib_agent_output, "stdlib.agent-output")?,
            stdlib_ai_agent_output: require_import(
                self.stdlib_ai_agent_output,
                "stdlib.ai-agent-output",
            )?,
            stdlib_ai_turn_next_input: require_import(
                self.stdlib_ai_turn_next_input,
                "stdlib.ai-turn-next-input",
            )?,
            stdlib_ai_turn_is_complete: require_import(
                self.stdlib_ai_turn_is_complete,
                "stdlib.ai-turn-is-complete",
            )?,
            stdlib_ai_turn_tool_count: require_import(
                self.stdlib_ai_turn_tool_count,
                "stdlib.ai-turn-tool-count",
            )?,
            stdlib_ai_turn_tool_args: require_import(
                self.stdlib_ai_turn_tool_args,
                "stdlib.ai-turn-tool-args",
            )?,
            stdlib_ai_turn_tool_index: require_import(
                self.stdlib_ai_turn_tool_index,
                "stdlib.ai-turn-tool-index",
            )?,
            stdlib_ai_tool_args_with_timeout: require_import(
                self.stdlib_ai_tool_args_with_timeout,
                "stdlib.ai-tool-args-with-timeout",
            )?,
            stdlib_ai_turn_add_result: require_import(
                self.stdlib_ai_turn_add_result,
                "stdlib.ai-turn-add-result",
            )?,
            stdlib_wait_timeout_error_envelope: require_import(
                self.stdlib_wait_timeout_error_envelope,
                "stdlib.wait-timeout-error-envelope",
            )?,
            stdlib_ai_turn_cache_key: require_import(
                self.stdlib_ai_turn_cache_key,
                "stdlib.ai-turn-cache-key",
            )?,
            stdlib_ai_turn_snapshot: require_import(
                self.stdlib_ai_turn_snapshot,
                "stdlib.ai-turn-snapshot",
            )?,
            stdlib_ai_turn_snapshot_part: require_import(
                self.stdlib_ai_turn_snapshot_part,
                "stdlib.ai-turn-snapshot-part",
            )?,
            stdlib_ai_turn_snapshot_tool_calls: require_import(
                self.stdlib_ai_turn_snapshot_tool_calls,
                "stdlib.ai-turn-snapshot-tool-calls",
            )?,
            stdlib_ai_turn_snapshot_complete: require_import(
                self.stdlib_ai_turn_snapshot_complete,
                "stdlib.ai-turn-snapshot-complete",
            )?,
            stdlib_ai_turn_output: require_import(
                self.stdlib_ai_turn_output,
                "stdlib.ai-turn-output",
            )?,
            stdlib_ai_tool_debug_start: require_import(
                self.stdlib_ai_tool_debug_start,
                "stdlib.ai-tool-debug-start",
            )?,
            stdlib_ai_tool_debug_end: require_import(
                self.stdlib_ai_tool_debug_end,
                "stdlib.ai-tool-debug-end",
            )?,
            stdlib_ai_memory_debug_start: require_import(
                self.stdlib_ai_memory_debug_start,
                "stdlib.ai-memory-debug-start",
            )?,
            stdlib_ai_memory_debug_end: require_import(
                self.stdlib_ai_memory_debug_end,
                "stdlib.ai-memory-debug-end",
            )?,
            stdlib_ai_memory_initial_state: require_import(
                self.stdlib_ai_memory_initial_state,
                "stdlib.ai-memory-initial-state",
            )?,
            stdlib_ai_memory_save_input: require_import(
                self.stdlib_ai_memory_save_input,
                "stdlib.ai-memory-save-input",
            )?,
            stdlib_ai_memory_compact_sliding: require_import(
                self.stdlib_ai_memory_compact_sliding,
                "stdlib.ai-memory-compact-sliding",
            )?,
            stdlib_ai_summarize_input: require_import(
                self.stdlib_ai_summarize_input,
                "stdlib.ai-summarize-input",
            )?,
            stdlib_ai_summarize_output: require_import(
                self.stdlib_ai_summarize_output,
                "stdlib.ai-summarize-output",
            )?,
            stdlib_agent_validate_input: require_import(
                self.stdlib_agent_validate_input,
                "stdlib.agent-validate-input",
            )?,
            stdlib_agent_connection_input: require_import(
                self.stdlib_agent_connection_input,
                "stdlib.agent-connection-input",
            )?,
            stdlib_agent_scope_input: require_import(
                self.stdlib_agent_scope_input,
                "stdlib.agent-scope-input",
            )?,
            stdlib_agent_tool_scope_input: require_import(
                self.stdlib_agent_tool_scope_input,
                "stdlib.agent-tool-scope-input",
            )?,
            stdlib_agent_cache_key: require_import(
                self.stdlib_agent_cache_key,
                "stdlib.agent-cache-key",
            )?,
            stdlib_agent_retry_sleep_key: require_import(
                self.stdlib_agent_retry_sleep_key,
                "stdlib.agent-retry-sleep-key",
            )?,
            stdlib_agent_attempt_result_key: require_import(
                self.stdlib_agent_attempt_result_key,
                "stdlib.agent-attempt-result-key",
            )?,
            stdlib_agent_attempt_envelope: require_import(
                self.stdlib_agent_attempt_envelope,
                "stdlib.agent-attempt-envelope",
            )?,
            stdlib_agent_retry_delay_ms: require_import(
                self.stdlib_agent_retry_delay_ms,
                "stdlib.agent-retry-delay-ms",
            )?,
            stdlib_agent_retry_error_info: require_import(
                self.stdlib_agent_retry_error_info,
                "stdlib.agent-retry-error-info",
            )?,
            stdlib_agent_error: require_import(self.stdlib_agent_error, "stdlib.agent-error")?,
            stdlib_agent_error_from_info: require_import(
                self.stdlib_agent_error_from_info,
                "stdlib.agent-error-from-info",
            )?,
            stdlib_agent_debug_error: require_import(
                self.stdlib_agent_debug_error,
                "stdlib.agent-debug-error",
            )?,
            stdlib_step_debug_start: require_import(
                self.stdlib_step_debug_start,
                "stdlib.step-debug-start",
            )?,
            stdlib_step_debug_end: require_import(
                self.stdlib_step_debug_end,
                "stdlib.step-debug-end",
            )?,
            stdlib_step_debug_error: require_import(
                self.stdlib_step_debug_error,
                "stdlib.step-debug-error",
            )?,
            agent_invokes: self.agent_invokes,
        })
    }
}

#[derive(Debug, Clone)]
pub(super) struct DirectCoreFunctionIndices {
    /// The top-level export shape the module is emitted against. Threaded
    /// through the indices because every lowerer already receives them, and
    /// the return convention at fail sites depends on it (tag under
    /// `wasi:cli/run`; result-area pointer under the invoke export).
    pub(super) abi: crate::direct_wasm::component::WorkflowAbi,
    /// Opt-in gate for the store-freeing durable-sleep lowering. When false
    /// (the default), a durable Delay blocks in the host on
    /// `durable-sleep-checkpoint` — byte-identical to the legacy path. When
    /// true AND `abi == InvokeHostImports`, the Delay checkpoints its deadline
    /// and exits with `outcome::suspended(at(deadline))` so the host frees the
    /// Store and reschedules a relaunch. Only meaningful under the invoke
    /// export (the only shape whose success arm can carry a wake).
    pub(super) store_freeing_sleep: bool,
    /// When true, the component imports no runtime; the terminal `complete`/
    /// `fail` are NOT lowered and the result travels solely via the invoke
    /// return value. Runtime index fields hold a poison sentinel and must never
    /// be called (see [`RUNTIME_OMITTED_POISON`]).
    pub(super) omit_runtime: bool,
    pub(super) runtime_load_input: u32,
    // (see `report_terminal_status` below for when complete/fail lower)
    pub(super) runtime_complete: u32,
    pub(super) runtime_fail: u32,
    pub(super) runtime_custom_event: u32,
    pub(super) runtime_debug_mode_enabled: u32,
    pub(super) runtime_breakpoint_pause: u32,
    pub(super) runtime_heartbeat: u32,
    pub(super) runtime_instance_id: u32,
    pub(super) runtime_is_cancelled: u32,
    pub(super) runtime_check_signals: u32,
    pub(super) runtime_poll_custom_signal: u32,
    pub(super) runtime_now_ms: u32,
    pub(super) runtime_get_checkpoint: u32,
    pub(super) runtime_checkpoint: u32,
    pub(super) runtime_handle_checkpoint_signal: u32,
    pub(super) runtime_record_retry_attempt: u32,
    pub(super) runtime_durable_sleep: u32,
    pub(super) runtime_blocking_sleep: u32,
    pub(super) runtime_durable_sleep_checkpoint: u32,
    pub(super) stdlib_init_manifest: u32,
    pub(super) stdlib_value_store_retain: u32,
    pub(super) stdlib_build_source: u32,
    pub(super) stdlib_apply_mapping: u32,
    pub(super) stdlib_eval_condition: u32,
    pub(super) stdlib_process_switch: u32,
    pub(super) stdlib_filter: u32,
    pub(super) stdlib_log_event: u32,
    pub(super) stdlib_log: u32,
    pub(super) stdlib_error_event: u32,
    pub(super) stdlib_error: u32,
    pub(super) stdlib_error_steps: u32,
    pub(super) stdlib_value_switch: u32,
    pub(super) stdlib_group_by: u32,
    pub(super) stdlib_split_item_count: u32,
    pub(super) stdlib_split_item: u32,
    pub(super) stdlib_split_iteration_variables: u32,
    pub(super) stdlib_split_validate_input: u32,
    pub(super) stdlib_split_validate_output: u32,
    pub(super) stdlib_split_initial_results: u32,
    pub(super) stdlib_split_append_output: u32,
    pub(super) stdlib_split_append_error: u32,
    pub(super) stdlib_split_output: u32,
    pub(super) stdlib_split_cache_key: u32,
    pub(super) stdlib_split_result: u32,
    pub(super) stdlib_split_output_from_result: u32,
    pub(super) stdlib_while_max_iterations: u32,
    pub(super) stdlib_while_initial_state: u32,
    pub(super) stdlib_while_condition_source: u32,
    pub(super) stdlib_while_condition: u32,
    pub(super) stdlib_while_iteration_variables: u32,
    pub(super) stdlib_while_advance_state: u32,
    pub(super) stdlib_while_output: u32,
    pub(super) stdlib_delay_duration_ms: u32,
    pub(super) stdlib_delay: u32,
    pub(super) stdlib_delay_sleep_key: u32,
    pub(super) stdlib_invoke_error_fields: u32,
    pub(super) stdlib_breakpoint_key: u32,
    pub(super) stdlib_breakpoint_event: u32,
    pub(super) stdlib_wait_signal_id: u32,
    pub(super) stdlib_wait_timeout_ms: u32,
    pub(super) stdlib_wait_timeout_error: u32,
    pub(super) stdlib_wait_on_wait_variables: u32,
    pub(super) stdlib_wait_on_wait_error: u32,
    pub(super) stdlib_wait_poll_interval_ms: u32,
    pub(super) stdlib_wait_event: u32,
    pub(super) stdlib_wait_debug_start: u32,
    pub(super) stdlib_wait_output: u32,
    pub(super) stdlib_ai_wait_tool_signal_id: u32,
    pub(super) stdlib_ai_wait_tool_result: u32,
    pub(super) stdlib_embed_workflow_cache_key: u32,
    pub(super) stdlib_embed_workflow_variables: u32,
    pub(super) stdlib_embed_workflow_result: u32,
    pub(super) stdlib_embed_workflow_output_from_result: u32,
    pub(super) stdlib_embed_workflow_error: u32,
    pub(super) stdlib_retry_sleep_key: u32,
    pub(super) stdlib_retry_delay_ms: u32,
    pub(super) stdlib_workflow_error_retryable: u32,
    pub(super) stdlib_workflow_error_rate_limited: u32,
    pub(super) stdlib_workflow_error_retry_after_ms: u32,
    pub(super) stdlib_agent_output: u32,
    pub(super) stdlib_ai_agent_output: u32,
    pub(super) stdlib_ai_turn_next_input: u32,
    pub(super) stdlib_ai_turn_is_complete: u32,
    pub(super) stdlib_ai_turn_tool_count: u32,
    pub(super) stdlib_ai_turn_tool_args: u32,
    pub(super) stdlib_ai_turn_tool_index: u32,
    pub(super) stdlib_ai_tool_args_with_timeout: u32,
    pub(super) stdlib_ai_turn_add_result: u32,
    pub(super) stdlib_wait_timeout_error_envelope: u32,
    pub(super) stdlib_ai_turn_cache_key: u32,
    pub(super) stdlib_ai_turn_snapshot: u32,
    pub(super) stdlib_ai_turn_snapshot_part: u32,
    pub(super) stdlib_ai_turn_snapshot_tool_calls: u32,
    pub(super) stdlib_ai_turn_snapshot_complete: u32,
    pub(super) stdlib_ai_turn_output: u32,
    pub(super) stdlib_ai_tool_debug_start: u32,
    pub(super) stdlib_ai_tool_debug_end: u32,
    pub(super) stdlib_ai_memory_debug_start: u32,
    pub(super) stdlib_ai_memory_debug_end: u32,
    pub(super) stdlib_ai_memory_initial_state: u32,
    pub(super) stdlib_ai_memory_save_input: u32,
    pub(super) stdlib_ai_memory_compact_sliding: u32,
    pub(super) stdlib_ai_summarize_input: u32,
    pub(super) stdlib_ai_summarize_output: u32,
    pub(super) stdlib_agent_validate_input: u32,
    pub(super) stdlib_agent_connection_input: u32,
    pub(super) stdlib_agent_scope_input: u32,
    pub(super) stdlib_agent_tool_scope_input: u32,
    pub(super) stdlib_agent_cache_key: u32,
    pub(super) stdlib_agent_retry_sleep_key: u32,
    pub(super) stdlib_agent_attempt_result_key: u32,
    pub(super) stdlib_agent_attempt_envelope: u32,
    pub(super) stdlib_agent_retry_delay_ms: u32,
    pub(super) stdlib_agent_retry_error_info: u32,
    pub(super) stdlib_agent_error: u32,
    pub(super) stdlib_agent_error_from_info: u32,
    pub(super) stdlib_agent_debug_error: u32,
    pub(super) stdlib_step_debug_start: u32,
    pub(super) stdlib_step_debug_end: u32,
    pub(super) stdlib_step_debug_error: u32,
    pub(super) agent_invokes: BTreeMap<String, DirectAgentInvokeImport>,
}

impl DirectCoreFunctionIndices {
    /// Whether the terminal `runtime.complete`/`runtime.fail` calls lower.
    ///
    /// Suppressed when the runtime is omitted (nothing to call) AND under the
    /// `AgentCapabilities` export even when the runtime IS imported (a durable
    /// workflow-agent): composed into a parent, the child shares the PARENT
    /// instance's runtime — its terminal `complete` would mark the parent's
    /// instance finished mid-flight. An agent capability's terminal result is
    /// the return value; instance lifecycle belongs to the caller. Non-terminal
    /// runtime calls (checkpoints, sleeps, signals, events) still lower.
    pub(super) fn report_terminal_status(&self) -> bool {
        !self.omit_runtime
            && !matches!(
                self.abi,
                crate::direct_wasm::component::WorkflowAbi::AgentCapabilities
            )
    }
}

#[derive(Debug, Clone)]
pub(super) struct DirectAgentInvokeImport {
    pub(super) function_index: u32,
    pub(super) params: Vec<WasmType>,
}

fn require_import(value: Option<u32>, name: &str) -> Result<u32, DirectCompileError> {
    value.ok_or_else(|| {
        DirectCompileError::Component(format!("missing {name} import in direct world"))
    })
}

/// Poison function index for a `runtime.*` slot when the component omits the
/// runtime import. The omit path (a pure, agent-shaped workflow) lowers NO
/// `runtime.*` call, so these slots are never referenced; if a lowerer emits
/// one anyway, the call targets an out-of-bounds function index and the
/// `ComponentEncoder::validate(true)` pass fails loudly at compile — a safety
/// net for a `needs_runtime` misclassification, never a silent miscompile.
const RUNTIME_OMITTED_POISON: u32 = u32::MAX;

/// `require_import` for a runtime function, tolerant of its absence under
/// `omit_runtime` (returns the poison index instead of erroring).
fn require_runtime(
    value: Option<u32>,
    name: &str,
    omit_runtime: bool,
) -> Result<u32, DirectCompileError> {
    if omit_runtime {
        Ok(value.unwrap_or(RUNTIME_OMITTED_POISON))
    } else {
        require_import(value, name)
    }
}

fn is_runtime_import(
    resolve: &Resolve,
    interface: Option<&WorldKey>,
    function: &WitFunction,
    function_name: &str,
) -> bool {
    function.name == function_name
        && interface
            .map(|key| resolve.name_world_key(key))
            .is_some_and(|name| name.starts_with("runtara:workflow-runtime/runtime"))
}

fn is_stdlib_import(
    resolve: &Resolve,
    interface: Option<&WorldKey>,
    function: &WitFunction,
    function_name: &str,
) -> bool {
    function.name == function_name
        && interface
            .map(|key| resolve.name_world_key(key))
            .is_some_and(|name| name.starts_with("runtara:workflow-stdlib/json"))
}

fn agent_id_for_import(resolve: &Resolve, interface: Option<&WorldKey>) -> Option<String> {
    let name = interface.map(|key| resolve.name_world_key(key))?;
    name.strip_prefix("runtara:agent-")?
        .split_once('/')
        .map(|(agent_id, _)| agent_id.to_string())
}

pub(super) fn is_wasi_cli_run_export(
    resolve: &Resolve,
    interface: Option<&WorldKey>,
    function: &WitFunction,
) -> bool {
    function.name == "run"
        && interface
            .map(|key| resolve.name_world_key(key))
            .is_some_and(|name| name.starts_with("wasi:cli/run"))
}

/// True for `runtara:workflow-lifecycle/lifecycle.invoke` — the entry export
/// under [`WorkflowAbi::InvokeHostImports`].
pub(super) fn is_lifecycle_invoke_export(
    resolve: &Resolve,
    interface: Option<&WorldKey>,
    function: &WitFunction,
) -> bool {
    function.name == "invoke"
        && interface
            .map(|key| resolve.name_world_key(key))
            .is_some_and(|name| name.starts_with("runtara:workflow-lifecycle/lifecycle"))
}

/// True for the workflow-as-agent capability export: an `invoke` in a
/// `runtara:agent-<id>/capabilities` interface. The compiled workflow is the
/// agent, so the package is `runtara:agent-*` (not a composed dependency).
pub(super) fn is_capabilities_invoke_export(
    resolve: &Resolve,
    interface: Option<&WorldKey>,
    function: &WitFunction,
) -> bool {
    function.name == "invoke"
        && interface
            .map(|key| resolve.name_world_key(key))
            .is_some_and(|name| {
                name.starts_with("runtara:agent-") && name.contains("/capabilities")
            })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn import_core_function(
    resolve: &Resolve,
    mangling: ManglingAndAbi,
    interface: Option<&WorldKey>,
    function: &WitFunction,
    function_index: u32,
    types: &mut TypeSection,
    type_count: &mut u32,
    imports: &mut ImportSection,
    import_indices: &mut DirectCoreImportIndices,
) {
    let signature = resolve.wasm_signature(mangling.import_variant(), function);
    let type_index = push_core_type(types, type_count, &signature.params, &signature.results);
    let (module, name) = resolve.wasm_import_name(
        mangling,
        WasmImport::Func {
            interface,
            func: function,
        },
    );
    imports.import(&module, &name, EntityType::Function(type_index));

    if is_runtime_import(resolve, interface, function, "load-input") {
        import_indices.runtime_load_input = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "complete") {
        import_indices.runtime_complete = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "fail") {
        import_indices.runtime_fail = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "custom-event") {
        import_indices.runtime_custom_event = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "debug-mode-enabled") {
        import_indices.runtime_debug_mode_enabled = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "breakpoint-pause") {
        import_indices.runtime_breakpoint_pause = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "heartbeat") {
        import_indices.runtime_heartbeat = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "instance-id") {
        import_indices.runtime_instance_id = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "is-cancelled") {
        import_indices.runtime_is_cancelled = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "check-signals") {
        import_indices.runtime_check_signals = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "poll-custom-signal") {
        import_indices.runtime_poll_custom_signal = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "now-ms") {
        import_indices.runtime_now_ms = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "get-checkpoint") {
        import_indices.runtime_get_checkpoint = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "checkpoint") {
        import_indices.runtime_checkpoint = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "handle-checkpoint-signal") {
        import_indices.runtime_handle_checkpoint_signal = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "record-retry-attempt") {
        import_indices.runtime_record_retry_attempt = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "durable-sleep") {
        import_indices.runtime_durable_sleep = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "blocking-sleep") {
        import_indices.runtime_blocking_sleep = Some(function_index);
    } else if is_runtime_import(resolve, interface, function, "durable-sleep-checkpoint") {
        import_indices.runtime_durable_sleep_checkpoint = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "init-manifest") {
        import_indices.stdlib_init_manifest = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "value-store-retain") {
        import_indices.stdlib_value_store_retain = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "build-source") {
        import_indices.stdlib_build_source = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "apply-mapping") {
        import_indices.stdlib_apply_mapping = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "eval-condition") {
        import_indices.stdlib_eval_condition = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "process-switch") {
        import_indices.stdlib_process_switch = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "filter") {
        import_indices.stdlib_filter = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "log-event") {
        import_indices.stdlib_log_event = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "log") {
        import_indices.stdlib_log = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "error-event") {
        import_indices.stdlib_error_event = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "error") {
        import_indices.stdlib_error = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "error-steps") {
        import_indices.stdlib_error_steps = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "value-switch") {
        import_indices.stdlib_value_switch = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "group-by") {
        import_indices.stdlib_group_by = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-item-count") {
        import_indices.stdlib_split_item_count = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-item") {
        import_indices.stdlib_split_item = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-iteration-variables") {
        import_indices.stdlib_split_iteration_variables = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-validate-input") {
        import_indices.stdlib_split_validate_input = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-validate-output") {
        import_indices.stdlib_split_validate_output = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-initial-results") {
        import_indices.stdlib_split_initial_results = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-append-output") {
        import_indices.stdlib_split_append_output = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-append-error") {
        import_indices.stdlib_split_append_error = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-output") {
        import_indices.stdlib_split_output = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-cache-key") {
        import_indices.stdlib_split_cache_key = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-result") {
        import_indices.stdlib_split_result = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "split-output-from-result") {
        import_indices.stdlib_split_output_from_result = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "while-max-iterations") {
        import_indices.stdlib_while_max_iterations = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "while-initial-state") {
        import_indices.stdlib_while_initial_state = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "while-condition-source") {
        import_indices.stdlib_while_condition_source = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "while-condition") {
        import_indices.stdlib_while_condition = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "while-iteration-variables") {
        import_indices.stdlib_while_iteration_variables = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "while-advance-state") {
        import_indices.stdlib_while_advance_state = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "while-output") {
        import_indices.stdlib_while_output = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "delay-duration-ms") {
        import_indices.stdlib_delay_duration_ms = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "delay") {
        import_indices.stdlib_delay = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "delay-sleep-key") {
        import_indices.stdlib_delay_sleep_key = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "invoke-error-fields") {
        import_indices.stdlib_invoke_error_fields = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "breakpoint-key") {
        import_indices.stdlib_breakpoint_key = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "breakpoint-event") {
        import_indices.stdlib_breakpoint_event = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "wait-signal-id") {
        import_indices.stdlib_wait_signal_id = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "wait-timeout-ms") {
        import_indices.stdlib_wait_timeout_ms = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "wait-timeout-error") {
        import_indices.stdlib_wait_timeout_error = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "wait-on-wait-variables") {
        import_indices.stdlib_wait_on_wait_variables = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "wait-on-wait-error") {
        import_indices.stdlib_wait_on_wait_error = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "wait-poll-interval-ms") {
        import_indices.stdlib_wait_poll_interval_ms = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "wait-event") {
        import_indices.stdlib_wait_event = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "wait-debug-start") {
        import_indices.stdlib_wait_debug_start = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "wait-output") {
        import_indices.stdlib_wait_output = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-wait-tool-signal-id") {
        import_indices.stdlib_ai_wait_tool_signal_id = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-wait-tool-result") {
        import_indices.stdlib_ai_wait_tool_result = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "embed-workflow-cache-key") {
        import_indices.stdlib_embed_workflow_cache_key = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "embed-workflow-variables") {
        import_indices.stdlib_embed_workflow_variables = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "embed-workflow-result") {
        import_indices.stdlib_embed_workflow_result = Some(function_index);
    } else if is_stdlib_import(
        resolve,
        interface,
        function,
        "embed-workflow-output-from-result",
    ) {
        import_indices.stdlib_embed_workflow_output_from_result = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "embed-workflow-error") {
        import_indices.stdlib_embed_workflow_error = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "retry-sleep-key") {
        import_indices.stdlib_retry_sleep_key = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "retry-delay-ms") {
        import_indices.stdlib_retry_delay_ms = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "workflow-error-retryable") {
        import_indices.stdlib_workflow_error_retryable = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "workflow-error-rate-limited") {
        import_indices.stdlib_workflow_error_rate_limited = Some(function_index);
    } else if is_stdlib_import(
        resolve,
        interface,
        function,
        "workflow-error-retry-after-ms",
    ) {
        import_indices.stdlib_workflow_error_retry_after_ms = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-output") {
        import_indices.stdlib_agent_output = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-agent-output") {
        import_indices.stdlib_ai_agent_output = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-turn-next-input") {
        import_indices.stdlib_ai_turn_next_input = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-turn-is-complete") {
        import_indices.stdlib_ai_turn_is_complete = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-turn-tool-count") {
        import_indices.stdlib_ai_turn_tool_count = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-turn-tool-args") {
        import_indices.stdlib_ai_turn_tool_args = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-turn-tool-index") {
        import_indices.stdlib_ai_turn_tool_index = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-tool-args-with-timeout") {
        import_indices.stdlib_ai_tool_args_with_timeout = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-turn-add-result") {
        import_indices.stdlib_ai_turn_add_result = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "wait-timeout-error-envelope") {
        import_indices.stdlib_wait_timeout_error_envelope = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-turn-cache-key") {
        import_indices.stdlib_ai_turn_cache_key = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-turn-snapshot") {
        import_indices.stdlib_ai_turn_snapshot = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-turn-snapshot-part") {
        import_indices.stdlib_ai_turn_snapshot_part = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-turn-snapshot-tool-calls") {
        import_indices.stdlib_ai_turn_snapshot_tool_calls = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-turn-snapshot-complete") {
        import_indices.stdlib_ai_turn_snapshot_complete = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-turn-output") {
        import_indices.stdlib_ai_turn_output = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-tool-debug-start") {
        import_indices.stdlib_ai_tool_debug_start = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-tool-debug-end") {
        import_indices.stdlib_ai_tool_debug_end = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-memory-debug-start") {
        import_indices.stdlib_ai_memory_debug_start = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-memory-debug-end") {
        import_indices.stdlib_ai_memory_debug_end = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-memory-initial-state") {
        import_indices.stdlib_ai_memory_initial_state = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-memory-save-input") {
        import_indices.stdlib_ai_memory_save_input = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-memory-compact-sliding") {
        import_indices.stdlib_ai_memory_compact_sliding = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-summarize-input") {
        import_indices.stdlib_ai_summarize_input = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "ai-summarize-output") {
        import_indices.stdlib_ai_summarize_output = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-validate-input") {
        import_indices.stdlib_agent_validate_input = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-connection-input") {
        import_indices.stdlib_agent_connection_input = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-scope-input") {
        import_indices.stdlib_agent_scope_input = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-tool-scope-input") {
        import_indices.stdlib_agent_tool_scope_input = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-cache-key") {
        import_indices.stdlib_agent_cache_key = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-retry-sleep-key") {
        import_indices.stdlib_agent_retry_sleep_key = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-attempt-result-key") {
        import_indices.stdlib_agent_attempt_result_key = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-attempt-envelope") {
        import_indices.stdlib_agent_attempt_envelope = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-retry-delay-ms") {
        import_indices.stdlib_agent_retry_delay_ms = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-error-info") {
        import_indices.stdlib_agent_error_info = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-retry-error-info") {
        import_indices.stdlib_agent_retry_error_info = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-error") {
        import_indices.stdlib_agent_error = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-error-from-info") {
        import_indices.stdlib_agent_error_from_info = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "agent-debug-error") {
        import_indices.stdlib_agent_debug_error = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "step-debug-start") {
        import_indices.stdlib_step_debug_start = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "step-debug-end") {
        import_indices.stdlib_step_debug_end = Some(function_index);
    } else if is_stdlib_import(resolve, interface, function, "step-debug-error") {
        import_indices.stdlib_step_debug_error = Some(function_index);
    } else if function.name == "invoke"
        && let Some(agent_id) = agent_id_for_import(resolve, interface)
    {
        import_indices.agent_invokes.insert(
            agent_id,
            DirectAgentInvokeImport {
                function_index,
                params: signature.params.clone(),
            },
        );
    }
}
