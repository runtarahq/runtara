// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! AiAgent tool-loop lowering for the direct workflow core Wasm emitter.
//!
//! Drives the `ai-tools` `chat-turn` capability: each turn appends the prior
//! round's tool results, runs the LLM, and reports `complete` (with the final
//! response) or `tools` (with the calls to dispatch). The core loop dispatches
//! each returned tool call back through the tool agent's `invoke`, feeds the
//! results into the next turn, and stops when the turn is complete (or the
//! iteration safety bound is hit). Conversation-state management lives in the
//! capability; this module is pure Wasm control flow.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    emit_retptr_error_or_return, load_agent_retptr_list, load_retptr_list, load_retptr_tag,
    push_retptr_arg, push_retptr_i32_load, push_retptr_u8_load, push_segment_args,
};
use super::agent_error::{
    emit_agent_invoke_capture_error_or_result, emit_agent_invoke_error_body_from_info,
    emit_agent_invoke_error_branch,
};
use super::agent_invoke::emit_agent_invoke;
use super::agent_retry::{
    emit_agent_advance_retry_attempt, emit_agent_capture_retry_sleep, emit_agent_retry_condition,
    emit_agent_retry_delay, emit_agent_retry_error_info, emit_agent_retry_sleep,
};
use super::checkpoint::{emit_checkpoint, emit_checkpoint_lookup, emit_checkpoint_save};
use super::debug::{
    emit_ai_memory_debug_event, emit_ai_tool_debug_event, emit_step_breakpoint,
    emit_step_debug_event,
};
use super::dispatcher::emit_run_plan_mapping;
use super::embed_workflow::emit_embed_workflow_tool_arm;
use super::mapping::{emit_apply_mapping, emit_build_source};
use super::split::emit_loop_iteration_heap_reset;
use super::wait::emit_ai_wait_tool_arm;
use super::{
    DIRECT_AGENT_ATTEMPT_ERR_FLAG_LOCAL, DIRECT_AGENT_RATE_LIMIT_WAIT_TOTAL_LOCAL,
    DIRECT_AGENT_RETRY_ATTEMPT_LOCAL, DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL,
    DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL, DIRECT_AI_BASE_LEN_LOCAL, DIRECT_AI_BASE_PTR_LOCAL,
    DIRECT_AI_CONV_LEN_LOCAL, DIRECT_AI_CONV_PTR_LOCAL, DIRECT_AI_HEAP_BASE_LOCAL,
    DIRECT_AI_ITER_LOCAL, DIRECT_AI_PENDING_LEN_LOCAL, DIRECT_AI_PENDING_PTR_LOCAL,
    DIRECT_AI_STATE_LEN_LOCAL, DIRECT_AI_STATE_PTR_LOCAL, DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
    DIRECT_AI_TOOL_ARGS_PTR_LOCAL, DIRECT_AI_TOOL_CALL_COUNTER_LOCAL, DIRECT_AI_TOOL_COUNT_LOCAL,
    DIRECT_AI_TOOL_IDX_LOCAL, DIRECT_AI_TOOL_MATCH_LOCAL, DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
    DIRECT_AI_TOOL_RESULT_PTR_LOCAL, DIRECT_AI_TURN_CHAIN_LOCAL, DIRECT_AI_TURN_INPUT_LEN_LOCAL,
    DIRECT_AI_TURN_INPUT_PTR_LOCAL, DIRECT_AI_TURN_OUT_LEN_LOCAL, DIRECT_AI_TURN_OUT_PTR_LOCAL,
    DIRECT_CHECKPOINT_FOUND_OFFSET, DIRECT_RET_BOOL_OK_OFFSET, DIRECT_RET_U32_OK_OFFSET,
    DirectCoreFunctionIndices, DirectCoreStaticData, DirectDataSegment, DirectRunPlan,
    DirectVariables,
};
use crate::direct_wasm::plan::{
    DirectAiMemoryPlan, DirectAiToolPlan, DirectErrorRoutePlan, DirectFailureTarget,
    DirectHandledTarget,
};

// Stdlib `ai-memory-debug-*` phase encoding (see runtara-workflow-stdlib.wit).
const AI_MEMORY_DEBUG_PHASE_LOAD: u32 = 0;
const AI_MEMORY_DEBUG_PHASE_SAVE: u32 = 1;
const AI_MEMORY_DEBUG_PHASE_COMPACT_SLIDING: u32 = 2;
const AI_MEMORY_DEBUG_PHASE_COMPACT_SUMMARIZE: u32 = 3;

// Inline children share these locals with an enclosing AI loop. Keep the
// caller's state on the guest operand stack; only the child's result escapes.
const CHILD_FRAME: [u32; 23] = [
    DIRECT_AI_BASE_PTR_LOCAL,
    DIRECT_AI_BASE_LEN_LOCAL,
    DIRECT_AI_STATE_PTR_LOCAL,
    DIRECT_AI_STATE_LEN_LOCAL,
    DIRECT_AI_PENDING_PTR_LOCAL,
    DIRECT_AI_PENDING_LEN_LOCAL,
    DIRECT_AI_TURN_OUT_PTR_LOCAL,
    DIRECT_AI_TURN_OUT_LEN_LOCAL,
    DIRECT_AI_TURN_INPUT_PTR_LOCAL,
    DIRECT_AI_TURN_INPUT_LEN_LOCAL,
    DIRECT_AI_TOOL_COUNT_LOCAL,
    DIRECT_AI_TOOL_IDX_LOCAL,
    DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
    DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
    DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
    DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
    DIRECT_AI_ITER_LOCAL,
    DIRECT_AI_TOOL_MATCH_LOCAL,
    DIRECT_AI_CONV_PTR_LOCAL,
    DIRECT_AI_CONV_LEN_LOCAL,
    DIRECT_AI_TOOL_CALL_COUNTER_LOCAL,
    DIRECT_AI_HEAP_BASE_LOCAL,
    DIRECT_AI_TURN_CHAIN_LOCAL,
];

pub(super) fn push_child_frame(body: &mut WasmFunction, result: (u32, u32)) {
    for local in CHILD_FRAME
        .into_iter()
        .filter(|local| *local != result.0 && *local != result.1)
    {
        body.instruction(&Instruction::LocalGet(local));
    }
}

pub(super) fn pop_child_frame(body: &mut WasmFunction, result: (u32, u32)) {
    for local in CHILD_FRAME
        .into_iter()
        .rev()
        .filter(|local| *local != result.0 && *local != result.1)
    {
        body.instruction(&Instruction::LocalSet(local));
    }
}

/// Retry policy for each chat-turn call: the AiAgent's `maxRetries` /
/// `retryDelay` and the workflow's rate-limit budget.
#[derive(Clone, Copy)]
pub(super) struct TurnRetry {
    pub(super) max_retries: u32,
    pub(super) retry_delay_ms: u64,
    pub(super) rate_limit_budget_ms: u64,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_ai_agent_loop_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    agent_id: u32,
    agent_component_id: &str,
    input_mapping_id: u32,
    durable_checkpoint: bool,
    breakpoint: bool,
    max_iterations: u32,
    tools: &[DirectAiToolPlan],
    memory: Option<&DirectAiMemoryPlan>,
    retry: TurnRetry,
    next_plan: &DirectRunPlan,
    error_plan: Option<&DirectErrorRoutePlan>,
    data_ptr_local: u32,
    data_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
    handled_target: Option<DirectHandledTarget>,
) {
    // Pause before any loop work — matching every other step's
    // "execution pauses before this step" breakpoint contract: the pause
    // lands before memory load and before the first LLM call.
    emit_step_breakpoint(
        body,
        indices,
        static_data,
        breakpoint,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
    );

    // Step debug-start for the AiAgent itself (the stdlib resolves the input
    // mapping into the event's inputs) — matching the single-shot path.
    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        true,
        step_id,
        source_ptr_local,
        source_len_local,
        route_ptr_local,
        route_len_local,
    );

    let turn_invoke = indices
        .agent_invoke(agent_component_id)
        .expect("AiAgent loop has a matching ai-tools component import");
    let turn_capability = static_data
        .agent_capability_id(agent_id)
        .expect("AiAgent loop has a static chat-turn capability id");
    let step_id_segment = static_data
        .step_id(step_id)
        .expect("run plan step ids are present in static data");
    let source = (source_ptr_local, source_len_local);
    let route = (route_ptr_local, route_len_local);

    // A finished step replays from its result, the fields its output is built
    // from: no memory load, model call, compaction or memory save. The rest
    // of the step is the result's miss arm, one block deeper.
    if durable_checkpoint {
        emit_ai_agent_key(
            body,
            indices,
            indices.stdlib_ai_agent_result_key,
            step_id_segment,
            source,
            route,
            (DIRECT_AI_TOOL_ARGS_PTR_LOCAL, DIRECT_AI_TOOL_ARGS_LEN_LOCAL),
        );
        emit_checkpoint_lookup(
            body,
            indices,
            DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
            DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
            DIRECT_AI_STATE_PTR_LOCAL,
            DIRECT_AI_STATE_LEN_LOCAL,
        );
        body.instruction(&Instruction::Else);
    }
    let failure_target = failure_target.map(|target| target.nested(u32::from(durable_checkpoint)));
    let handled_target = handled_target.map(|target| target.nested(u32::from(durable_checkpoint)));

    // Build the constant base turn config from the input mapping.
    emit_apply_mapping(
        body,
        indices,
        input_mapping_id,
        source_ptr_local,
        source_len_local,
        DIRECT_AI_BASE_PTR_LOCAL,
        DIRECT_AI_BASE_LEN_LOCAL,
        None,
    );

    // Conversation memory: resolve the conversation id once, then load prior
    // history into the initial loop state. Without memory the initial state is
    // an empty object (chat-turn defaults chatHistory/[]/0).
    if let Some(memory) = memory {
        // conversation = apply(conversation_mapping, source)
        emit_apply_mapping(
            body,
            indices,
            memory.conversation_mapping_id,
            source_ptr_local,
            source_len_local,
            DIRECT_AI_CONV_PTR_LOCAL,
            DIRECT_AI_CONV_LEN_LOCAL,
            None,
        );
        // Memory-load debug-start ({step}.memory_load), like the generated loop.
        emit_ai_memory_debug_event(
            body,
            indices,
            static_data,
            track_events,
            agent_id,
            AI_MEMORY_DEBUG_PHASE_LOAD,
            true,
            DIRECT_AI_CONV_PTR_LOCAL,
            DIRECT_AI_CONV_LEN_LOCAL,
            None,
            None,
            0,
            source_ptr_local,
            source_len_local,
            DIRECT_AI_TURN_INPUT_PTR_LOCAL,
            DIRECT_AI_TURN_INPUT_LEN_LOCAL,
        );
        // load_output = invoke load-memory(conversation)
        let load_invoke = indices
            .agent_invoke(&memory.agent_component_id)
            .expect("AiAgent memory provider has a matching component import");
        let load_capability = static_data
            .agent_capability_id(memory.load_agent_id)
            .expect("AiAgent memory load has a static capability id");
        if let Some(timeout) = memory.timeout_ms {
            super::agent_call_deadline::enter_memory(
                body,
                indices,
                static_data,
                memory.load_agent_id,
                &memory.step_id,
                memory.durable,
                timeout,
                (source_ptr_local, source_len_local),
            );
        }
        let memory_depth = u32::from(memory.durable && memory.timeout_ms.is_some());
        emit_agent_invoke(
            body,
            indices,
            load_invoke,
            load_capability,
            static_data,
            memory.load_agent_id,
            DIRECT_AI_CONV_PTR_LOCAL,
            DIRECT_AI_CONV_LEN_LOCAL,
            source_ptr_local,
            source_len_local,
            super::agent_invoke::AgentInvocationSite::MemoryLoad,
        );
        super::deadline_scope::propagate(
            body,
            indices,
            failure_target.map(|target| target.nested(memory_depth)),
        );
        emit_agent_invoke_error_branch(
            body,
            indices,
            static_data,
            track_events,
            memory.load_agent_id,
            step_id,
            output_ptr_local,
            output_len_local,
            source_ptr_local,
            source_len_local,
            steps_ptr_local,
            steps_len_local,
            error_plan,
            route_ptr_local,
            route_len_local,
            variables,
            data_ptr_local,
            data_len_local,
            workflow_log_kind,
            workflow_error_kind,
            failure_target.map(|target| target.nested(memory_depth)),
            handled_target.map(|target| target.nested(memory_depth)),
        );
        load_agent_retptr_list(
            body,
            DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
            DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
        );
        if memory.timeout_ms.is_some() {
            super::agent_call_deadline::finish(body, indices, memory.durable);
        }
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_RESULT_PTR_LOCAL));
        body.instruction(&Instruction::LocalSet(DIRECT_AI_TURN_OUT_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_RESULT_LEN_LOCAL));
        body.instruction(&Instruction::LocalSet(DIRECT_AI_TURN_OUT_LEN_LOCAL));
        // state = ai-memory-initial-state(load_output)
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_LEN_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_ai_memory_initial_state));
        emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
        load_retptr_list(body, DIRECT_AI_STATE_PTR_LOCAL, DIRECT_AI_STATE_LEN_LOCAL);
        // Memory-load debug-end with the loaded history's count + previews.
        emit_ai_memory_debug_event(
            body,
            indices,
            static_data,
            track_events,
            agent_id,
            AI_MEMORY_DEBUG_PHASE_LOAD,
            false,
            DIRECT_AI_CONV_PTR_LOCAL,
            DIRECT_AI_CONV_LEN_LOCAL,
            Some((DIRECT_AI_STATE_PTR_LOCAL, DIRECT_AI_STATE_LEN_LOCAL)),
            None,
            0,
            source_ptr_local,
            source_len_local,
            DIRECT_AI_TURN_INPUT_PTR_LOCAL,
            DIRECT_AI_TURN_INPUT_LEN_LOCAL,
        );
    } else {
        set_segment(
            body,
            &static_data.agent_empty_parameters,
            DIRECT_AI_STATE_PTR_LOCAL,
            DIRECT_AI_STATE_LEN_LOCAL,
        );
    }
    set_segment(
        body,
        &static_data.split_empty_results,
        DIRECT_AI_PENDING_PTR_LOCAL,
        DIRECT_AI_PENDING_LEN_LOCAL,
    );
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(DIRECT_AI_ITER_LOCAL));
    // Monotonic per-tool-call counter (across turns), folded into a
    // WaitForSignal-tool's signal id so repeated calls get distinct ids.
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(DIRECT_AI_TOOL_CALL_COUNTER_LOCAL));

    // Capture the heap watermark above the pre-loop persistent buffers (base turn
    // config, conversation, initial state/pending). Each turn's scratch — the
    // model turn input, the chat-turn output, every tool result — is bump-allocated
    // above this and reclaimed at the top of the next turn (see the per-turn reset
    // below). Without it a long conversation grows guest memory per turn, the same
    // unbounded-bump-allocator leak the Split/While arena reset fixed.
    body.instruction(&Instruction::GlobalGet(0));
    body.instruction(&Instruction::LocalSet(DIRECT_AI_HEAP_BASE_LOCAL));

    body.instruction(&Instruction::Block(BlockType::Empty)); // $outer
    if durable_checkpoint {
        emit_restore_latest_turn_slot(body, indices, step_id_segment, source, route);
    }
    body.instruction(&Instruction::Loop(BlockType::Empty)); // $turn

    // Safety bound: break to output when the turn budget is exhausted.
    body.instruction(&Instruction::LocalGet(DIRECT_AI_ITER_LOCAL));
    body.instruction(&Instruction::I32Const(max_iterations as i32));
    body.instruction(&Instruction::I32GeU);
    body.instruction(&Instruction::BrIf(1)); // Br $outer
    body.instruction(&Instruction::LocalGet(DIRECT_AI_ITER_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_AI_ITER_LOCAL));

    // Reclaim the previous turn's scratch before this turn allocates anything.
    // The only heap buffers that must survive into the next turn are the loop
    // state (the growing conversation) and pending (this turn's tool results);
    // bundle them into one snapshot, compact it down to the watermark, rewind the
    // bump pointer (freeing the prior turn's model input/output and tool-result
    // buffers), then unpack the two survivors back out. This mirrors the
    // Split/While single-survivor arena reset, using the snapshot as the single
    // survivor. The tool-call counter rides an i32 local, so it needs no
    // relocation; `complete` is never read back here, so pass false. Both the
    // live path and the durable replay path (which `Br $turn`s back here) funnel
    // through this point, so neither grows guest memory per turn.
    body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_PENDING_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_PENDING_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_CALL_COUNTER_LOCAL));
    body.instruction(&Instruction::I32Const(0)); // complete = false (unused on unpack)
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_ai_turn_snapshot));
    emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
    load_retptr_list(
        body,
        DIRECT_AI_TURN_INPUT_PTR_LOCAL,
        DIRECT_AI_TURN_INPUT_LEN_LOCAL,
    );
    emit_loop_iteration_heap_reset(
        body,
        DIRECT_AI_HEAP_BASE_LOCAL,
        DIRECT_AI_TURN_INPUT_PTR_LOCAL,
        DIRECT_AI_TURN_INPUT_LEN_LOCAL,
    );
    // state = snapshot.state (part 0); allocated just above the compacted snapshot.
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_LEN_LOCAL));
    body.instruction(&Instruction::I32Const(0));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_ai_turn_snapshot_part));
    emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
    load_retptr_list(body, DIRECT_AI_STATE_PTR_LOCAL, DIRECT_AI_STATE_LEN_LOCAL);
    // pending = snapshot.pending (part 1); allocated above the relocated state.
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_LEN_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_ai_turn_snapshot_part));
    emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
    load_retptr_list(
        body,
        DIRECT_AI_PENDING_PTR_LOCAL,
        DIRECT_AI_PENDING_LEN_LOCAL,
    );

    // A store that kept both slots holds per-turn snapshots for the turns
    // after the newer one: follow them until one is missing. A hit restores
    // the snapshot's state, pending tool results and tool counter without a
    // model call or tool dispatch; a completing turn's snapshot exits the
    // loop. Depths inside the hit: $turn = 2, $outer = 3.
    if durable_checkpoint {
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_CHAIN_LOCAL));
        body.instruction(&Instruction::If(BlockType::Empty));
        emit_ai_turn_key(
            body,
            indices,
            indices.stdlib_ai_turn_cache_key,
            step_id_segment,
            KeyTurn::Current,
            source,
            route,
        );
        emit_checkpoint_lookup(
            body,
            indices,
            DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
            DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
            DIRECT_AI_TURN_INPUT_PTR_LOCAL,
            DIRECT_AI_TURN_INPUT_LEN_LOCAL,
        );
        emit_restore_turn_snapshot(body, indices, route);
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_LEN_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_ai_turn_snapshot_complete));
        emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
        push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
        body.instruction(&Instruction::BrIf(3)); // Br $outer
        body.instruction(&Instruction::Br(2)); // Br $turn
        body.instruction(&Instruction::Else);
        // The turns after a missing one are live.
        body.instruction(&Instruction::I32Const(0));
        body.instruction(&Instruction::LocalSet(DIRECT_AI_TURN_CHAIN_LOCAL));
        body.instruction(&Instruction::End); // lookup
        body.instruction(&Instruction::End); // chain
    }

    if durable_checkpoint {
        push_segment_args(body, step_id_segment);
        body.instruction(&Instruction::LocalGet(DIRECT_AI_ITER_LOCAL));
        body.instruction(&Instruction::LocalGet(source_ptr_local));
        body.instruction(&Instruction::LocalGet(source_len_local));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_ai_turn_response_key));
        emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
        load_retptr_list(
            body,
            DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
            DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
        );
        emit_checkpoint_lookup(
            body,
            indices,
            DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
            DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
            DIRECT_AI_TURN_OUT_PTR_LOCAL,
            DIRECT_AI_TURN_OUT_LEN_LOCAL,
        );
        // TOOL_MATCH is unused until dispatch. Preserve whether this response
        // needs a first write without retaining another heap or stack frame.
        body.instruction(&Instruction::I32Const(0));
        body.instruction(&Instruction::LocalSet(DIRECT_AI_TOOL_MATCH_LOCAL));
        body.instruction(&Instruction::Else);
    }
    let turn_failure_depth = 2 + u32::from(durable_checkpoint);
    // turn_input = ai-turn-next-input(base, state, pending)
    body.instruction(&Instruction::LocalGet(DIRECT_AI_BASE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_BASE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_PENDING_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_PENDING_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_ai_turn_next_input));
    emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
    load_retptr_list(
        body,
        DIRECT_AI_TURN_INPUT_PTR_LOCAL,
        DIRECT_AI_TURN_INPUT_LEN_LOCAL,
    );

    // turn_out = invoke chat-turn(turn_input)
    let invoke_turn = |body: &mut WasmFunction| {
        emit_agent_invoke(
            body,
            indices,
            turn_invoke,
            turn_capability,
            static_data,
            agent_id,
            DIRECT_AI_TURN_INPUT_PTR_LOCAL,
            DIRECT_AI_TURN_INPUT_LEN_LOCAL,
            source_ptr_local,
            source_len_local,
            super::agent_invoke::AgentInvocationSite::AiTurn,
        );
    };
    if retry.max_retries == 0 {
        invoke_turn(body);
        super::deadline_scope::propagate(
            body,
            indices,
            failure_target.map(|target| target.nested(turn_failure_depth)),
        );
        emit_agent_invoke_error_branch(
            body,
            indices,
            static_data,
            track_events,
            agent_id,
            step_id,
            output_ptr_local,
            output_len_local,
            source_ptr_local,
            source_len_local,
            steps_ptr_local,
            steps_len_local,
            error_plan,
            route_ptr_local,
            route_len_local,
            variables,
            data_ptr_local,
            data_len_local,
            workflow_log_kind,
            workflow_error_kind,
            // Inside Block($outer) + Loop($turn): rejoining handlers and Split
            // failure collectors must also leave a pending-response cache miss.
            failure_target.map(|target| target.nested(turn_failure_depth)),
            handled_target.map(|target| target.nested(turn_failure_depth)),
        );
    } else {
        // Retry this turn's LLM call in-run, with the Agent step's retry
        // state machine: a retryable failure sleeps and calls again; the
        // last failure routes like an unretried one. Each turn starts with
        // a fresh attempt count and rate-limit wait total.
        body.instruction(&Instruction::I32Const(1));
        body.instruction(&Instruction::LocalSet(DIRECT_AGENT_RETRY_ATTEMPT_LOCAL));
        body.instruction(&Instruction::I64Const(0));
        body.instruction(&Instruction::LocalSet(
            DIRECT_AGENT_RATE_LIMIT_WAIT_TOTAL_LOCAL,
        ));
        body.instruction(&Instruction::Block(BlockType::Empty));
        body.instruction(&Instruction::Loop(BlockType::Empty));
        invoke_turn(body);
        super::deadline_scope::propagate(
            body,
            indices,
            failure_target.map(|target| target.nested(turn_failure_depth + 2)),
        );
        load_retptr_tag(body);
        body.instruction(&Instruction::LocalSet(DIRECT_AGENT_ATTEMPT_ERR_FLAG_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AGENT_ATTEMPT_ERR_FLAG_LOCAL));
        body.instruction(&Instruction::If(BlockType::Empty));
        emit_agent_capture_retry_sleep(body);
        emit_agent_retry_error_info(
            body,
            indices,
            DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL,
            DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL,
        );
        emit_agent_retry_condition(
            body,
            retry.max_retries,
            retry.retry_delay_ms,
            retry.rate_limit_budget_ms,
        );
        body.instruction(&Instruction::If(BlockType::Empty));
        emit_agent_advance_retry_attempt(body);
        emit_agent_retry_delay(
            body,
            indices,
            retry.max_retries,
            retry.retry_delay_ms,
            retry.rate_limit_budget_ms,
        );
        emit_agent_retry_sleep(body, indices);
        body.instruction(&Instruction::Br(2)); // Br $retry
        body.instruction(&Instruction::End);
        emit_agent_invoke_error_body_from_info(
            body,
            indices,
            static_data,
            track_events,
            agent_id,
            step_id,
            output_ptr_local,
            output_len_local,
            DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL,
            DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL,
            source_ptr_local,
            source_len_local,
            steps_ptr_local,
            steps_len_local,
            error_plan,
            route_ptr_local,
            route_len_local,
            variables,
            data_ptr_local,
            data_len_local,
            workflow_log_kind,
            workflow_error_kind,
            failure_target.map(|target| target.nested(turn_failure_depth + 3)),
            handled_target.map(|target| target.nested(turn_failure_depth + 3)),
        );
        body.instruction(&Instruction::End); // failed attempt
        load_agent_retptr_list(
            body,
            DIRECT_AI_TURN_OUT_PTR_LOCAL,
            DIRECT_AI_TURN_OUT_LEN_LOCAL,
        );
        body.instruction(&Instruction::Br(1)); // leave the retry block
        body.instruction(&Instruction::End); // $retry
        body.instruction(&Instruction::End); // retry block
    }
    if retry.max_retries == 0 {
        load_agent_retptr_list(
            body,
            DIRECT_AI_TURN_OUT_PTR_LOCAL,
            DIRECT_AI_TURN_OUT_LEN_LOCAL,
        );
    }

    if durable_checkpoint {
        body.instruction(&Instruction::I32Const(1));
        body.instruction(&Instruction::LocalSet(DIRECT_AI_TOOL_MATCH_LOCAL));
        body.instruction(&Instruction::End); // pending-response lookup
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_LEN_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_ai_turn_response_validate));
        emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_MATCH_LOCAL));
        body.instruction(&Instruction::If(BlockType::Empty));
        emit_checkpoint_save(
            body,
            indices,
            DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
            DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
            DIRECT_AI_TURN_OUT_PTR_LOCAL,
            DIRECT_AI_TURN_OUT_LEN_LOCAL,
        );
        body.instruction(&Instruction::End);
    }

    // Carry the turn output forward as the next turn's loop state; reset pending.
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_AI_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_AI_STATE_LEN_LOCAL));
    set_segment(
        body,
        &static_data.split_empty_results,
        DIRECT_AI_PENDING_PTR_LOCAL,
        DIRECT_AI_PENDING_LEN_LOCAL,
    );

    // if ai-turn-is-complete(turn_out): break to output. The completing turn
    // saves no snapshot: replay restores the turn before it and takes this
    // turn's response from its row, which stays until the step's result.
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_ai_turn_is_complete));
    emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::BrIf(1)); // Br $outer

    // Dispatch each requested tool call.
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_ai_turn_tool_count));
    emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
    push_retptr_i32_load(body, DIRECT_RET_U32_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_AI_TOOL_COUNT_LOCAL));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(DIRECT_AI_TOOL_IDX_LOCAL));

    body.instruction(&Instruction::Block(BlockType::Empty)); // $tools_done
    body.instruction(&Instruction::Loop(BlockType::Empty)); // $tool_iter
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_IDX_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_COUNT_LOCAL));
    body.instruction(&Instruction::I32GeU);
    body.instruction(&Instruction::BrIf(1)); // Br $tools_done

    // args = ai-turn-tool-args(turn_out, idx)
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_IDX_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_ai_turn_tool_args));
    emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
    load_retptr_list(
        body,
        DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
        DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
    );

    // Resolve which tool this call selects.
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_IDX_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_ai_turn_tool_index));
    emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
    push_retptr_i32_load(body, DIRECT_RET_U32_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_AI_TOOL_MATCH_LOCAL));

    // Step debug-start for this tool call — the synthetic
    // `{step}.tool.{name}.{call}` step, matching the generated loop. The
    // TURN_INPUT scratch is free during dispatch (last used to build the
    // chat-turn input, reused for snapshots after).
    emit_ai_tool_debug_event(
        body,
        indices,
        static_data,
        track_events,
        agent_id,
        DIRECT_AI_TURN_OUT_PTR_LOCAL,
        DIRECT_AI_TURN_OUT_LEN_LOCAL,
        DIRECT_AI_TOOL_IDX_LOCAL,
        DIRECT_AI_ITER_LOCAL,
        DIRECT_AI_TOOL_CALL_COUNTER_LOCAL,
        None,
        source_ptr_local,
        source_len_local,
        DIRECT_AI_TURN_INPUT_PTR_LOCAL,
        DIRECT_AI_TURN_INPUT_LEN_LOCAL,
    );

    // Default the tool result to an empty list so an unknown tool index is a
    // benign no-op (the model gets an empty result).
    set_segment(
        body,
        &static_data.split_empty_results,
        DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
        DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
    );

    // Dispatch by tool index: `if match == i { run tools[i] }`.
    let caller_agent_id = agent_id;
    for (tool_index, tool) in tools.iter().enumerate() {
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_MATCH_LOCAL));
        body.instruction(&Instruction::I32Const(tool_index as i32));
        body.instruction(&Instruction::I32Eq);
        body.instruction(&Instruction::If(BlockType::Empty));
        match tool {
            DirectAiToolPlan::Agent {
                agent_id,
                agent_component_id,
                step_id: tool_step_id,
                durable,
                label,
                timeout_ms,
            } => {
                let tool_invoke = indices
                    .agent_invoke(agent_component_id)
                    .expect("AiAgent tool has a matching component import");
                let tool_capability = static_data
                    .agent_capability_id(*agent_id)
                    .expect("AiAgent tool has a static capability id");
                if let Some(timeout) = timeout_ms {
                    super::agent_call_deadline::enter_tool(
                        body,
                        indices,
                        static_data,
                        *agent_id,
                        tool_step_id,
                        step_id,
                        label,
                        *durable,
                        *timeout,
                        (source_ptr_local, source_len_local),
                    );
                }
                // A workflow-agent tool shares this instance's checkpoint
                // store: wrap its arguments in the namespace envelope with a
                // PER-CALL scope ({ai_step}.tool.{label}.{counter}) — one
                // tool can be dispatched many times in one loop, so the
                // per-step scope would collide across calls. The counter is
                // the replay-stable tool-call counter (restored from the
                // turn snapshot), so replays re-derive identical scopes.
                // No-op for native tool targets.
                if static_data.agent_is_workflow_agent(*agent_id) {
                    let ai_step_segment = static_data
                        .step_id(step_id)
                        .expect("AiAgent step id is present in static data");
                    let label_segment = static_data
                        .step_id(label)
                        .expect("AiAgent tool label is interned in static data");
                    push_segment_args(body, ai_step_segment);
                    push_segment_args(body, label_segment);
                    body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_CALL_COUNTER_LOCAL));
                    body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_ARGS_PTR_LOCAL));
                    body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_ARGS_LEN_LOCAL));
                    body.instruction(&Instruction::LocalGet(source_ptr_local));
                    body.instruction(&Instruction::LocalGet(source_len_local));
                    push_retptr_arg(body);
                    body.instruction(&Instruction::Call(indices.stdlib_agent_tool_scope_input));
                    emit_retptr_error_or_return(
                        body,
                        indices,
                        None,
                        route_ptr_local,
                        route_len_local,
                    );
                    load_retptr_list(
                        body,
                        DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
                        DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
                    );
                }
                emit_agent_invoke(
                    body,
                    indices,
                    tool_invoke,
                    tool_capability,
                    static_data,
                    *agent_id,
                    DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
                    DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
                    source_ptr_local,
                    source_len_local,
                    super::agent_invoke::AgentInvocationSite::AiTool(caller_agent_id),
                );
                super::deadline_scope::propagate(
                    body,
                    indices,
                    failure_target.map(|target| {
                        target.nested(5 + u32::from(*durable && timeout_ms.is_some()))
                    }),
                );
                // A tool failure is fed back to the LLM as the tool result (the
                // error envelope) and the loop continues, rather than failing the
                // workflow — matching the generated loop's `{"error": …}` result.
                emit_agent_invoke_capture_error_or_result(
                    body,
                    indices,
                    *agent_id,
                    DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
                    DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
                );
                if timeout_ms.is_some() {
                    super::agent_call_deadline::finish(body, indices, *durable);
                }
            }
            DirectAiToolPlan::Embed {
                step_id: embed_step_id,
                input_mapping_id,
                label,
                durable,
                timeout_ms,
                child_plan,
            } => {
                emit_embed_workflow_tool_arm(
                    body,
                    indices,
                    static_data,
                    track_events,
                    embed_step_id,
                    *input_mapping_id,
                    step_id,
                    label,
                    *durable,
                    *timeout_ms,
                    child_plan,
                    DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
                    DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
                    DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
                    DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
                    steps_ptr_local,
                    steps_len_local,
                    source_ptr_local,
                    source_len_local,
                    route_ptr_local,
                    route_len_local,
                    workflow_log_kind,
                    workflow_error_kind,
                );
            }
            DirectAiToolPlan::Wait {
                step_id: wait_step_id,
                label,
            } => {
                emit_ai_wait_tool_arm(
                    body,
                    indices,
                    static_data,
                    step_id,
                    wait_step_id,
                    label,
                    DIRECT_AI_TOOL_CALL_COUNTER_LOCAL,
                    DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
                    DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
                    source_ptr_local,
                    source_len_local,
                    output_ptr_local,
                    output_len_local,
                );
            }
        }
        // An inherited deadline also bypasses embedded-tool error feedback.
        super::deadline_scope::propagate(
            body,
            indices,
            failure_target.map(|target| target.nested(5)),
        );
        body.instruction(&Instruction::End);
    }

    // Step debug-end for this tool call, carrying the dispatched result (or
    // the error envelope a failed tool feeds back to the model).
    emit_ai_tool_debug_event(
        body,
        indices,
        static_data,
        track_events,
        agent_id,
        DIRECT_AI_TURN_OUT_PTR_LOCAL,
        DIRECT_AI_TURN_OUT_LEN_LOCAL,
        DIRECT_AI_TOOL_IDX_LOCAL,
        DIRECT_AI_ITER_LOCAL,
        DIRECT_AI_TOOL_CALL_COUNTER_LOCAL,
        Some((
            DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
            DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
        )),
        source_ptr_local,
        source_len_local,
        DIRECT_AI_TURN_INPUT_PTR_LOCAL,
        DIRECT_AI_TURN_INPUT_LEN_LOCAL,
    );

    // pending = ai-turn-add-result(pending, turn_out, idx, tool_result)
    body.instruction(&Instruction::LocalGet(DIRECT_AI_PENDING_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_PENDING_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_IDX_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_RESULT_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_RESULT_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_ai_turn_add_result));
    emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
    load_retptr_list(
        body,
        DIRECT_AI_PENDING_PTR_LOCAL,
        DIRECT_AI_PENDING_LEN_LOCAL,
    );

    // Bump the monotonic per-tool-call counter after dispatching this call (the
    // WaitForSignal-tool arm above read it for this call's signal id).
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_CALL_COUNTER_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_AI_TOOL_CALL_COUNTER_LOCAL));

    body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_IDX_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_AI_TOOL_IDX_LOCAL));
    body.instruction(&Instruction::Br(0)); // continue $tool_iter
    body.instruction(&Instruction::End); // $tool_iter
    body.instruction(&Instruction::End); // $tools_done

    // Snapshot the turn (the model's response in `state`, the dispatched tool
    // results in `pending`, the advanced tool counter) in its slot before
    // looping: replay restores it instead of running the turn again, and the
    // rows it replaces go. Depths here: $turn = 0, $outer = 1.
    if durable_checkpoint {
        emit_save_turn_slot(body, indices, step_id_segment, source, route);
    }

    body.instruction(&Instruction::Br(0)); // continue $turn
    body.instruction(&Instruction::End); // $turn
    body.instruction(&Instruction::End); // $outer

    // Conversation memory: save the final conversation history.
    if let Some(memory) = memory {
        let compact_phase = if memory.summarize.is_some() {
            AI_MEMORY_DEBUG_PHASE_COMPACT_SUMMARIZE
        } else {
            AI_MEMORY_DEBUG_PHASE_COMPACT_SLIDING
        };
        // Stash the pre-compaction state (its buffer stays valid) so the
        // debug-end can report before/after counts; TOOL_ARGS is free here.
        body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_PTR_LOCAL));
        body.instruction(&Instruction::LocalSet(DIRECT_AI_TOOL_ARGS_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_LEN_LOCAL));
        body.instruction(&Instruction::LocalSet(DIRECT_AI_TOOL_ARGS_LEN_LOCAL));
        // Compaction debug-start ({step}.memory.compact); the stdlib returns
        // an empty payload below the threshold and the event is skipped —
        // matching the generated "only when exceeded" gate.
        emit_ai_memory_debug_event(
            body,
            indices,
            static_data,
            track_events,
            agent_id,
            compact_phase,
            true,
            DIRECT_AI_CONV_PTR_LOCAL,
            DIRECT_AI_CONV_LEN_LOCAL,
            Some((DIRECT_AI_STATE_PTR_LOCAL, DIRECT_AI_STATE_LEN_LOCAL)),
            None,
            memory.max_messages,
            source_ptr_local,
            source_len_local,
            DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
            DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
        );
        // Compaction before save. Generated always compacts when memory is
        // configured (default window 50).
        if let Some(summarize) = memory.summarize.as_ref() {
            // Summarize strategy: state = summarize-memory(ai-summarize-input(
            // base, state, max_messages)). The capability LLM-summarizes the
            // oldest messages (or no-ops below the threshold) and returns the
            // compacted state. A durable step saves the capability's output,
            // so replay never bills the summary twice. The TOOL_RESULT
            // scratch is free between the compaction debug events.
            let summary_key = (
                DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
                DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
            );
            if durable_checkpoint {
                emit_ai_agent_key(
                    body,
                    indices,
                    indices.stdlib_ai_summary_key,
                    step_id_segment,
                    source,
                    route,
                    summary_key,
                );
                emit_checkpoint_lookup(
                    body,
                    indices,
                    summary_key.0,
                    summary_key.1,
                    DIRECT_AI_TURN_OUT_PTR_LOCAL,
                    DIRECT_AI_TURN_OUT_LEN_LOCAL,
                );
                body.instruction(&Instruction::Else);
            }
            let summary_depth = u32::from(durable_checkpoint);
            body.instruction(&Instruction::LocalGet(DIRECT_AI_BASE_PTR_LOCAL));
            body.instruction(&Instruction::LocalGet(DIRECT_AI_BASE_LEN_LOCAL));
            body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_PTR_LOCAL));
            body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_LEN_LOCAL));
            body.instruction(&Instruction::I32Const(memory.max_messages as i32));
            push_retptr_arg(body);
            body.instruction(&Instruction::Call(indices.stdlib_ai_summarize_input));
            emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
            load_retptr_list(
                body,
                DIRECT_AI_TURN_INPUT_PTR_LOCAL,
                DIRECT_AI_TURN_INPUT_LEN_LOCAL,
            );
            let summarize_invoke = indices
                .agent_invoke(&summarize.agent_component_id)
                .expect("AiAgent summarize provider has a matching component import");
            let summarize_capability = static_data
                .agent_capability_id(summarize.agent_id)
                .expect("AiAgent summarize has a static capability id");
            emit_agent_invoke(
                body,
                indices,
                summarize_invoke,
                summarize_capability,
                static_data,
                summarize.agent_id,
                DIRECT_AI_TURN_INPUT_PTR_LOCAL,
                DIRECT_AI_TURN_INPUT_LEN_LOCAL,
                source_ptr_local,
                source_len_local,
                super::agent_invoke::AgentInvocationSite::Summarize,
            );
            super::deadline_scope::propagate(
                body,
                indices,
                failure_target.map(|target| target.nested(summary_depth)),
            );
            emit_agent_invoke_error_branch(
                body,
                indices,
                static_data,
                track_events,
                summarize.agent_id,
                step_id,
                output_ptr_local,
                output_len_local,
                source_ptr_local,
                source_len_local,
                steps_ptr_local,
                steps_len_local,
                error_plan,
                route_ptr_local,
                route_len_local,
                variables,
                data_ptr_local,
                data_len_local,
                workflow_log_kind,
                workflow_error_kind,
                failure_target.map(|target| target.nested(summary_depth)),
                handled_target.map(|target| target.nested(summary_depth)),
            );
            load_agent_retptr_list(
                body,
                DIRECT_AI_TURN_OUT_PTR_LOCAL,
                DIRECT_AI_TURN_OUT_LEN_LOCAL,
            );
            if durable_checkpoint {
                // The call may have reused the scratch the key was in.
                emit_ai_agent_key(
                    body,
                    indices,
                    indices.stdlib_ai_summary_key,
                    step_id_segment,
                    source,
                    route,
                    summary_key,
                );
                emit_checkpoint_save(
                    body,
                    indices,
                    summary_key.0,
                    summary_key.1,
                    DIRECT_AI_TURN_OUT_PTR_LOCAL,
                    DIRECT_AI_TURN_OUT_LEN_LOCAL,
                );
                body.instruction(&Instruction::End);
            }
            // state = ai-summarize-output(summarize_out)
            body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_PTR_LOCAL));
            body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_LEN_LOCAL));
            push_retptr_arg(body);
            body.instruction(&Instruction::Call(indices.stdlib_ai_summarize_output));
            emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
            load_retptr_list(body, DIRECT_AI_STATE_PTR_LOCAL, DIRECT_AI_STATE_LEN_LOCAL);
        } else {
            // Sliding-window (default): state = ai-memory-compact-sliding(state,
            // max_messages).
            body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_PTR_LOCAL));
            body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_LEN_LOCAL));
            body.instruction(&Instruction::I32Const(memory.max_messages as i32));
            push_retptr_arg(body);
            body.instruction(&Instruction::Call(indices.stdlib_ai_memory_compact_sliding));
            emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
            load_retptr_list(body, DIRECT_AI_STATE_PTR_LOCAL, DIRECT_AI_STATE_LEN_LOCAL);
        }

        // Compaction debug-end with before/after counts (and the summary for
        // the summarize strategy); skipped below the threshold like the start.
        emit_ai_memory_debug_event(
            body,
            indices,
            static_data,
            track_events,
            agent_id,
            compact_phase,
            false,
            DIRECT_AI_CONV_PTR_LOCAL,
            DIRECT_AI_CONV_LEN_LOCAL,
            Some((DIRECT_AI_STATE_PTR_LOCAL, DIRECT_AI_STATE_LEN_LOCAL)),
            Some((DIRECT_AI_TOOL_ARGS_PTR_LOCAL, DIRECT_AI_TOOL_ARGS_LEN_LOCAL)),
            memory.max_messages,
            source_ptr_local,
            source_len_local,
            DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
            DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
        );

        // Memory-save debug-start ({step}.memory_save).
        emit_ai_memory_debug_event(
            body,
            indices,
            static_data,
            track_events,
            agent_id,
            AI_MEMORY_DEBUG_PHASE_SAVE,
            true,
            DIRECT_AI_CONV_PTR_LOCAL,
            DIRECT_AI_CONV_LEN_LOCAL,
            Some((DIRECT_AI_STATE_PTR_LOCAL, DIRECT_AI_STATE_LEN_LOCAL)),
            None,
            0,
            source_ptr_local,
            source_len_local,
            DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
            DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
        );

        // save_input = ai-memory-save-input(conversation, final_state)
        body.instruction(&Instruction::LocalGet(DIRECT_AI_CONV_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_CONV_LEN_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_LEN_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_ai_memory_save_input));
        emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
        load_retptr_list(
            body,
            DIRECT_AI_TURN_INPUT_PTR_LOCAL,
            DIRECT_AI_TURN_INPUT_LEN_LOCAL,
        );
        // invoke save-memory(save_input); the result is unused.
        let save_invoke = indices
            .agent_invoke(&memory.agent_component_id)
            .expect("AiAgent memory provider has a matching component import");
        let save_capability = static_data
            .agent_capability_id(memory.save_agent_id)
            .expect("AiAgent memory save has a static capability id");
        if let Some(timeout) = memory.timeout_ms {
            super::agent_call_deadline::enter_memory(
                body,
                indices,
                static_data,
                memory.save_agent_id,
                &memory.step_id,
                memory.durable,
                timeout,
                (source_ptr_local, source_len_local),
            );
        }
        let memory_depth = u32::from(memory.durable && memory.timeout_ms.is_some());
        emit_agent_invoke(
            body,
            indices,
            save_invoke,
            save_capability,
            static_data,
            memory.save_agent_id,
            DIRECT_AI_TURN_INPUT_PTR_LOCAL,
            DIRECT_AI_TURN_INPUT_LEN_LOCAL,
            source_ptr_local,
            source_len_local,
            super::agent_invoke::AgentInvocationSite::MemorySave,
        );
        super::deadline_scope::propagate(
            body,
            indices,
            failure_target.map(|target| target.nested(memory_depth)),
        );
        emit_agent_invoke_error_branch(
            body,
            indices,
            static_data,
            track_events,
            memory.save_agent_id,
            step_id,
            output_ptr_local,
            output_len_local,
            source_ptr_local,
            source_len_local,
            steps_ptr_local,
            steps_len_local,
            error_plan,
            route_ptr_local,
            route_len_local,
            variables,
            data_ptr_local,
            data_len_local,
            workflow_log_kind,
            workflow_error_kind,
            failure_target.map(|target| target.nested(memory_depth)),
            handled_target.map(|target| target.nested(memory_depth)),
        );
        load_agent_retptr_list(
            body,
            DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
            DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
        );
        if memory.timeout_ms.is_some() {
            super::agent_call_deadline::finish(body, indices, memory.durable);
        }

        // Memory-save debug-end (failures took the agent-error branch above).
        emit_ai_memory_debug_event(
            body,
            indices,
            static_data,
            track_events,
            agent_id,
            AI_MEMORY_DEBUG_PHASE_SAVE,
            false,
            DIRECT_AI_CONV_PTR_LOCAL,
            DIRECT_AI_CONV_LEN_LOCAL,
            Some((DIRECT_AI_STATE_PTR_LOCAL, DIRECT_AI_STATE_LEN_LOCAL)),
            None,
            0,
            source_ptr_local,
            source_len_local,
            DIRECT_AI_TURN_OUT_PTR_LOCAL,
            DIRECT_AI_TURN_OUT_LEN_LOCAL,
        );
    }

    // Save the step's result and close its miss arm. Recording it deletes the
    // step's last slot, its responses, its memory summary and its memory
    // calls: one row remains.
    if durable_checkpoint {
        body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_LEN_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_ai_agent_result));
        emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
        load_retptr_list(body, DIRECT_AI_STATE_PTR_LOCAL, DIRECT_AI_STATE_LEN_LOCAL);
        emit_ai_agent_key(
            body,
            indices,
            indices.stdlib_ai_agent_result_key,
            step_id_segment,
            source,
            route,
            (DIRECT_AI_TOOL_ARGS_PTR_LOCAL, DIRECT_AI_TOOL_ARGS_LEN_LOCAL),
        );
        emit_checkpoint_save(
            body,
            indices,
            DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
            DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
            DIRECT_AI_STATE_PTR_LOCAL,
            DIRECT_AI_STATE_LEN_LOCAL,
        );
        body.instruction(&Instruction::End);
    }

    // Build the AiAgent step output from the final (complete or at-bound)
    // turn, or from the result a replay found.
    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_ai_turn_output));
    emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
    load_retptr_list(body, steps_ptr_local, steps_len_local);

    emit_build_source(
        body,
        indices,
        variables,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        None,
    );

    // Step debug-end for the AiAgent: the rebuilt source now carries the
    // `{response, iterations, toolCalls}` envelope under `steps.{id}`.
    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        false,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );

    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        variables,
        next_plan,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
        None,
        None,
    );
}

/// The turn a turn key names: the loop's current one, or a fixed slot.
#[derive(Clone, Copy)]
enum KeyTurn {
    Current,
    Slot(i32),
}

/// key = `key_function(step_id, turn, source)`, into the TOOL_ARGS scratch.
#[allow(clippy::too_many_arguments)]
fn emit_ai_turn_key(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    key_function: u32,
    step_id_segment: &DirectDataSegment,
    turn: KeyTurn,
    source: (u32, u32),
    route: (u32, u32),
) {
    push_segment_args(body, step_id_segment);
    match turn {
        KeyTurn::Current => body.instruction(&Instruction::LocalGet(DIRECT_AI_ITER_LOCAL)),
        KeyTurn::Slot(slot) => body.instruction(&Instruction::I32Const(slot)),
    };
    body.instruction(&Instruction::LocalGet(source.0));
    body.instruction(&Instruction::LocalGet(source.1));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(key_function));
    emit_retptr_error_or_return(body, indices, None, route.0, route.1);
    load_retptr_list(
        body,
        DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
        DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
    );
}

/// key = `key_function(step_id, source)`, into `key`.
fn emit_ai_agent_key(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    key_function: u32,
    step_id_segment: &DirectDataSegment,
    source: (u32, u32),
    route: (u32, u32),
    key: (u32, u32),
) {
    push_segment_args(body, step_id_segment);
    body.instruction(&Instruction::LocalGet(source.0));
    body.instruction(&Instruction::LocalGet(source.1));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(key_function));
    emit_retptr_error_or_return(body, indices, None, route.0, route.1);
    load_retptr_list(body, key.0, key.1);
}

/// Restore the loop's state, pending tool results and tool-call counter from
/// the turn snapshot in TURN_INPUT. The counter must survive replay because
/// WaitForSignal-tool signal ids embed it.
fn emit_restore_turn_snapshot(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    route: (u32, u32),
) {
    for (part, (ptr, len)) in [
        (0, (DIRECT_AI_STATE_PTR_LOCAL, DIRECT_AI_STATE_LEN_LOCAL)),
        (
            1,
            (DIRECT_AI_PENDING_PTR_LOCAL, DIRECT_AI_PENDING_LEN_LOCAL),
        ),
    ] {
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_LEN_LOCAL));
        body.instruction(&Instruction::I32Const(part));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_ai_turn_snapshot_part));
        emit_retptr_error_or_return(body, indices, None, route.0, route.1);
        load_retptr_list(body, ptr, len);
    }
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(
        indices.stdlib_ai_turn_snapshot_tool_calls,
    ));
    emit_retptr_error_or_return(body, indices, None, route.0, route.1);
    push_retptr_i32_load(body, DIRECT_RET_U32_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_AI_TOOL_CALL_COUNTER_LOCAL));
}

/// Replay restores the newer of the step's two turn slots before its first
/// turn, and sets the turn counter to it; a fresh step finds both empty.
/// Both slots hold a turn only where the store kept the older one when the
/// newer was saved (an address too long to collapse); the turns after it are
/// then per-turn snapshots, and the chain local says to follow them.
fn emit_restore_latest_turn_slot(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    step_id_segment: &DirectDataSegment,
    source: (u32, u32),
    route: (u32, u32),
) {
    for (slot, (ptr, len)) in [
        (
            0,
            (
                DIRECT_AI_TURN_INPUT_PTR_LOCAL,
                DIRECT_AI_TURN_INPUT_LEN_LOCAL,
            ),
        ),
        (
            1,
            (DIRECT_AI_TURN_OUT_PTR_LOCAL, DIRECT_AI_TURN_OUT_LEN_LOCAL),
        ),
    ] {
        // An empty list stands for an empty slot.
        body.instruction(&Instruction::I32Const(0));
        body.instruction(&Instruction::LocalSet(ptr));
        body.instruction(&Instruction::I32Const(0));
        body.instruction(&Instruction::LocalSet(len));
        emit_ai_turn_key(
            body,
            indices,
            indices.stdlib_ai_turn_slot_key,
            step_id_segment,
            KeyTurn::Slot(slot),
            source,
            route,
        );
        emit_checkpoint_lookup(
            body,
            indices,
            DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
            DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
            ptr,
            len,
        );
        body.instruction(&Instruction::End);
    }
    for len in [DIRECT_AI_TURN_INPUT_LEN_LOCAL, DIRECT_AI_TURN_OUT_LEN_LOCAL] {
        body.instruction(&Instruction::LocalGet(len));
        body.instruction(&Instruction::I32Const(0));
        body.instruction(&Instruction::I32Ne);
    }
    body.instruction(&Instruction::I32And);
    body.instruction(&Instruction::LocalSet(DIRECT_AI_TURN_CHAIN_LOCAL));

    // snapshot = ai-turn-latest(slot 0, slot 1) → TURN_INPUT
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_ai_turn_latest));
    emit_retptr_error_or_return(body, indices, None, route.0, route.1);
    load_retptr_list(
        body,
        DIRECT_AI_TURN_INPUT_PTR_LOCAL,
        DIRECT_AI_TURN_INPUT_LEN_LOCAL,
    );
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_LEN_LOCAL));
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_restore_turn_snapshot(body, indices, route);
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_ai_turn_snapshot_turn));
    emit_retptr_error_or_return(body, indices, None, route.0, route.1);
    push_retptr_i32_load(body, DIRECT_RET_U32_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_AI_ITER_LOCAL));
    body.instruction(&Instruction::End);
}

/// Save the turn whose tools were just dispatched in its slot. Recording the
/// slot deletes the other one, this step's model responses and its tool
/// calls. Where the store kept the other slot instead (the write finds this
/// slot taken by an older turn), the turn is also saved under its per-turn
/// key, which replay then follows; signals are handled once, after both.
fn emit_save_turn_slot(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    step_id_segment: &DirectDataSegment,
    source: (u32, u32),
    route: (u32, u32),
) {
    emit_ai_turn_key(
        body,
        indices,
        indices.stdlib_ai_turn_slot_key,
        step_id_segment,
        KeyTurn::Current,
        source,
        route,
    );
    // snapshot = ai-turn-slot-snapshot(state, pending, counter, turn)
    body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_PENDING_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_PENDING_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_CALL_COUNTER_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_ITER_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_ai_turn_slot_snapshot));
    emit_retptr_error_or_return(body, indices, None, route.0, route.1);
    load_retptr_list(
        body,
        DIRECT_AI_TURN_INPUT_PTR_LOCAL,
        DIRECT_AI_TURN_INPUT_LEN_LOCAL,
    );
    let record = |body: &mut WasmFunction| {
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_ARGS_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_ARGS_LEN_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_LEN_LOCAL));
        emit_checkpoint(body, indices);
    };
    record(body);
    push_retptr_u8_load(body, DIRECT_CHECKPOINT_FOUND_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_ai_turn_key(
        body,
        indices,
        indices.stdlib_ai_turn_cache_key,
        step_id_segment,
        KeyTurn::Current,
        source,
        route,
    );
    record(body);
    body.instruction(&Instruction::End);
    super::cooperative_wait::emit_checkpoint_signal(body, indices);
}

/// Load a static data segment's offset/length into a pointer/length local pair.
fn set_segment(
    body: &mut WasmFunction,
    segment: &DirectDataSegment,
    ptr_local: u32,
    len_local: u32,
) {
    body.instruction(&Instruction::I32Const(segment.offset));
    body.instruction(&Instruction::LocalSet(ptr_local));
    body.instruction(&Instruction::I32Const(segment.len_i32()));
    body.instruction(&Instruction::LocalSet(len_local));
}
