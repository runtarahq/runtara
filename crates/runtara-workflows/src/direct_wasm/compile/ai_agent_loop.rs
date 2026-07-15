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
    emit_retptr_error_or_return, load_agent_retptr_list, load_retptr_list, push_retptr_arg,
    push_retptr_i32_load, push_retptr_u8_load, push_segment_args,
};
use super::agent_error::{
    emit_agent_invoke_capture_error_or_result, emit_agent_invoke_error_branch,
};
use super::agent_invoke::emit_agent_invoke;
use super::checkpoint::{emit_checkpoint_lookup, emit_checkpoint_save};
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
    DIRECT_AI_BASE_LEN_LOCAL, DIRECT_AI_BASE_PTR_LOCAL, DIRECT_AI_CONV_LEN_LOCAL,
    DIRECT_AI_CONV_PTR_LOCAL, DIRECT_AI_HEAP_BASE_LOCAL, DIRECT_AI_ITER_LOCAL,
    DIRECT_AI_PENDING_LEN_LOCAL, DIRECT_AI_PENDING_PTR_LOCAL, DIRECT_AI_STATE_LEN_LOCAL,
    DIRECT_AI_STATE_PTR_LOCAL, DIRECT_AI_TOOL_ARGS_LEN_LOCAL, DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
    DIRECT_AI_TOOL_CALL_COUNTER_LOCAL, DIRECT_AI_TOOL_COUNT_LOCAL, DIRECT_AI_TOOL_IDX_LOCAL,
    DIRECT_AI_TOOL_MATCH_LOCAL, DIRECT_AI_TOOL_RESULT_LEN_LOCAL, DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
    DIRECT_AI_TURN_INPUT_LEN_LOCAL, DIRECT_AI_TURN_INPUT_PTR_LOCAL, DIRECT_AI_TURN_OUT_LEN_LOCAL,
    DIRECT_AI_TURN_OUT_PTR_LOCAL, DIRECT_RET_BOOL_OK_OFFSET, DIRECT_RET_U32_OK_OFFSET,
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
        .agent_invokes
        .get(agent_component_id)
        .expect("AiAgent loop has a matching ai-tools component import");
    let turn_capability = static_data
        .agent_capability_id(agent_id)
        .expect("AiAgent loop has a static chat-turn capability id");
    let step_id_segment = static_data
        .step_id(step_id)
        .expect("run plan step ids are present in static data");
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
            .agent_invokes
            .get(&memory.agent_component_id)
            .expect("AiAgent memory provider has a matching component import");
        let load_capability = static_data
            .agent_capability_id(memory.load_agent_id)
            .expect("AiAgent memory load has a static capability id");
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
            failure_target,
            handled_target,
        );
        load_agent_retptr_list(
            body,
            DIRECT_AI_TURN_OUT_PTR_LOCAL,
            DIRECT_AI_TURN_OUT_LEN_LOCAL,
        );
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

    // Per-turn durability (GAP-04): a completed turn's snapshot under
    // `{step}.turn.{n}` replays without re-running (and re-billing) the LLM
    // call or its tool dispatches. On a hit the snapshot's state/pending/
    // tool-counter are restored; a completing turn's snapshot exits the loop
    // directly. Depths at this point: $turn = 0, $outer = 1.
    if durable_checkpoint {
        // key = ai-turn-cache-key(step_id, iter, source) → TOOL_ARGS scratch
        push_segment_args(body, step_id_segment);
        body.instruction(&Instruction::LocalGet(DIRECT_AI_ITER_LOCAL));
        body.instruction(&Instruction::LocalGet(source_ptr_local));
        body.instruction(&Instruction::LocalGet(source_len_local));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_ai_turn_cache_key));
        emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
        load_retptr_list(
            body,
            DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
            DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
        );
        // Opens If(found); snapshot lands in TURN_INPUT scratch. Inside the
        // If, depths shift: $turn = 1, $outer = 2.
        emit_checkpoint_lookup(
            body,
            indices,
            DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
            DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
            DIRECT_AI_TURN_INPUT_PTR_LOCAL,
            DIRECT_AI_TURN_INPUT_LEN_LOCAL,
        );
        // state = snapshot.state
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_LEN_LOCAL));
        body.instruction(&Instruction::I32Const(0));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_ai_turn_snapshot_part));
        emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
        load_retptr_list(body, DIRECT_AI_STATE_PTR_LOCAL, DIRECT_AI_STATE_LEN_LOCAL);
        // pending = snapshot.pending
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
        // tool_counter = snapshot.toolCalls (WaitForSignal-tool signal ids
        // embed it, so replayed ids must keep advancing from where they were)
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_LEN_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(
            indices.stdlib_ai_turn_snapshot_tool_calls,
        ));
        emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
        push_retptr_i32_load(body, DIRECT_RET_U32_OK_OFFSET);
        body.instruction(&Instruction::LocalSet(DIRECT_AI_TOOL_CALL_COUNTER_LOCAL));
        // complete? → exit the loop with the restored state
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_INPUT_LEN_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_ai_turn_snapshot_complete));
        emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
        push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
        body.instruction(&Instruction::BrIf(2)); // Br $outer
        // Not complete: continue with the next turn (its own lookup decides
        // whether it replays or runs live).
        body.instruction(&Instruction::Br(1)); // Br $turn
        body.instruction(&Instruction::End); // close lookup If(found)
    }

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
        // failure collectors must branch out through two extra blocks.
        failure_target.map(|target| target.nested(2)),
        handled_target.map(|target| target.nested(2)),
    );
    load_agent_retptr_list(
        body,
        DIRECT_AI_TURN_OUT_PTR_LOCAL,
        DIRECT_AI_TURN_OUT_LEN_LOCAL,
    );

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

    // if ai-turn-is-complete(turn_out): snapshot the completing turn (so a
    // crash after completion but before downstream checkpoints replays it
    // without a model call), then break to output. Inside the If, depths
    // shift: $turn = 1, $outer = 2.
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TURN_OUT_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_ai_turn_is_complete));
    emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    if durable_checkpoint {
        // key = ai-turn-cache-key(step_id, iter, source) → TOOL_ARGS scratch
        push_segment_args(body, step_id_segment);
        body.instruction(&Instruction::LocalGet(DIRECT_AI_ITER_LOCAL));
        body.instruction(&Instruction::LocalGet(source_ptr_local));
        body.instruction(&Instruction::LocalGet(source_len_local));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_ai_turn_cache_key));
        emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
        load_retptr_list(
            body,
            DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
            DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
        );
        // snapshot = ai-turn-snapshot(state, pending, counter, complete)
        body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_LEN_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_PENDING_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_PENDING_LEN_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_CALL_COUNTER_LOCAL));
        body.instruction(&Instruction::I32Const(1));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_ai_turn_snapshot));
        emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
        load_retptr_list(
            body,
            DIRECT_AI_TURN_INPUT_PTR_LOCAL,
            DIRECT_AI_TURN_INPUT_LEN_LOCAL,
        );
        emit_checkpoint_save(
            body,
            indices,
            DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
            DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
            DIRECT_AI_TURN_INPUT_PTR_LOCAL,
            DIRECT_AI_TURN_INPUT_LEN_LOCAL,
        );
    }
    body.instruction(&Instruction::Br(2)); // Br $outer
    body.instruction(&Instruction::End);

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
    for (tool_index, tool) in tools.iter().enumerate() {
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_MATCH_LOCAL));
        body.instruction(&Instruction::I32Const(tool_index as i32));
        body.instruction(&Instruction::I32Eq);
        body.instruction(&Instruction::If(BlockType::Empty));
        match tool {
            DirectAiToolPlan::Agent {
                agent_id,
                agent_component_id,
                timeout_ms,
            } => {
                let tool_invoke = indices
                    .agent_invokes
                    .get(agent_component_id)
                    .expect("AiAgent tool has a matching component import");
                let tool_capability = static_data
                    .agent_capability_id(*agent_id)
                    .expect("AiAgent tool has a static capability id");
                // Inject the tool step's own timeout into the model-provided
                // arguments so the dispatched call is bounded independently of
                // the AiAgent turnTimeout. Only when the tool step set one;
                // otherwise the tool capability's own default applies. The
                // merge overwrites DIRECT_AI_TOOL_ARGS in place.
                if let Some(ms) = timeout_ms {
                    body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_ARGS_PTR_LOCAL));
                    body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_ARGS_LEN_LOCAL));
                    body.instruction(&Instruction::I64Const(*ms as i64));
                    push_retptr_arg(body);
                    body.instruction(&Instruction::Call(indices.stdlib_ai_tool_args_with_timeout));
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
            }
            DirectAiToolPlan::Embed {
                step_id,
                child_plan,
            } => {
                emit_embed_workflow_tool_arm(
                    body,
                    indices,
                    static_data,
                    track_events,
                    step_id,
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

    // Per-turn durability: snapshot the completed turn (LLM response in
    // `state`, dispatched tool results in `pending`, advanced tool counter)
    // before looping — replaying this iteration restores instead of
    // re-running. Depths here: $turn = 0, $outer = 1.
    if durable_checkpoint {
        // key = ai-turn-cache-key(step_id, iter, source) → TOOL_ARGS scratch
        push_segment_args(body, step_id_segment);
        body.instruction(&Instruction::LocalGet(DIRECT_AI_ITER_LOCAL));
        body.instruction(&Instruction::LocalGet(source_ptr_local));
        body.instruction(&Instruction::LocalGet(source_len_local));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_ai_turn_cache_key));
        emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
        load_retptr_list(
            body,
            DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
            DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
        );
        // snapshot = ai-turn-snapshot(state, pending, counter, complete)
        body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_STATE_LEN_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_PENDING_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_PENDING_LEN_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_CALL_COUNTER_LOCAL));
        body.instruction(&Instruction::I32Const(0));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_ai_turn_snapshot));
        emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
        load_retptr_list(
            body,
            DIRECT_AI_TURN_INPUT_PTR_LOCAL,
            DIRECT_AI_TURN_INPUT_LEN_LOCAL,
        );
        emit_checkpoint_save(
            body,
            indices,
            DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
            DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
            DIRECT_AI_TURN_INPUT_PTR_LOCAL,
            DIRECT_AI_TURN_INPUT_LEN_LOCAL,
        );
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
            // compacted state.
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
                .agent_invokes
                .get(&summarize.agent_component_id)
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
                failure_target,
                handled_target,
            );
            load_agent_retptr_list(
                body,
                DIRECT_AI_TURN_OUT_PTR_LOCAL,
                DIRECT_AI_TURN_OUT_LEN_LOCAL,
            );
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
            .agent_invokes
            .get(&memory.agent_component_id)
            .expect("AiAgent memory provider has a matching component import");
        let save_capability = static_data
            .agent_capability_id(memory.save_agent_id)
            .expect("AiAgent memory save has a static capability id");
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
            failure_target,
            handled_target,
        );
        load_agent_retptr_list(
            body,
            DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
            DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
        );
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

    // Build the AiAgent step output from the final (complete or at-bound) turn.
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
