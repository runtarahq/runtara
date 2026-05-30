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
    push_retptr_i32_load, push_retptr_u8_load,
};
use super::agent_error::emit_agent_invoke_error_branch;
use super::agent_invoke::emit_agent_invoke;
use super::dispatcher::emit_run_plan_mapping;
use super::mapping::{emit_apply_mapping, emit_build_source};
use super::{
    DIRECT_AI_BASE_LEN_LOCAL, DIRECT_AI_BASE_PTR_LOCAL, DIRECT_AI_ITER_LOCAL,
    DIRECT_AI_PENDING_LEN_LOCAL, DIRECT_AI_PENDING_PTR_LOCAL, DIRECT_AI_STATE_LEN_LOCAL,
    DIRECT_AI_STATE_PTR_LOCAL, DIRECT_AI_TOOL_ARGS_LEN_LOCAL, DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
    DIRECT_AI_TOOL_COUNT_LOCAL, DIRECT_AI_TOOL_IDX_LOCAL, DIRECT_AI_TOOL_MATCH_LOCAL,
    DIRECT_AI_TOOL_RESULT_LEN_LOCAL, DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
    DIRECT_AI_TURN_INPUT_LEN_LOCAL, DIRECT_AI_TURN_INPUT_PTR_LOCAL, DIRECT_AI_TURN_OUT_LEN_LOCAL,
    DIRECT_AI_TURN_OUT_PTR_LOCAL, DIRECT_RET_BOOL_OK_OFFSET, DIRECT_RET_U32_OK_OFFSET,
    DirectCoreFunctionIndices, DirectCoreStaticData, DirectDataSegment, DirectRunPlan,
    DirectVariables,
};
use crate::direct_wasm::plan::DirectAiToolPlan;

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
    max_iterations: u32,
    tools: &[DirectAiToolPlan],
    next_plan: &DirectRunPlan,
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
) {
    let turn_invoke = indices
        .agent_invokes
        .get(agent_component_id)
        .expect("AiAgent loop has a matching ai-tools component import");
    let turn_capability = static_data
        .agent_capability_id(agent_id)
        .expect("AiAgent loop has a static chat-turn capability id");
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

    // Initial loop state: an empty object (chat-turn defaults chatHistory/[]/0),
    // empty pending results, zero turns.
    set_segment(
        body,
        &static_data.agent_empty_parameters,
        DIRECT_AI_STATE_PTR_LOCAL,
        DIRECT_AI_STATE_LEN_LOCAL,
    );
    set_segment(
        body,
        &static_data.split_empty_results,
        DIRECT_AI_PENDING_PTR_LOCAL,
        DIRECT_AI_PENDING_LEN_LOCAL,
    );
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(DIRECT_AI_ITER_LOCAL));

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
        turn_invoke,
        turn_capability,
        static_data,
        agent_id,
        DIRECT_AI_TURN_INPUT_PTR_LOCAL,
        DIRECT_AI_TURN_INPUT_LEN_LOCAL,
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
        None,
        route_ptr_local,
        route_len_local,
        variables,
        data_ptr_local,
        data_len_local,
        workflow_log_kind,
        workflow_error_kind,
        None,
        None,
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

    // if ai-turn-is-complete(turn_out): break to output.
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

    // Default the tool result to an empty list so an unknown tool index is a
    // benign no-op (the model gets an empty result).
    set_segment(
        body,
        &static_data.split_empty_results,
        DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
        DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
    );

    // Dispatch by tool index: `if match == i { invoke tools[i] }`.
    for (tool_index, tool) in tools.iter().enumerate() {
        let tool_invoke = indices
            .agent_invokes
            .get(&tool.agent_component_id)
            .expect("AiAgent tool has a matching component import");
        let tool_capability = static_data
            .agent_capability_id(tool.agent_id)
            .expect("AiAgent tool has a static capability id");

        body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_MATCH_LOCAL));
        body.instruction(&Instruction::I32Const(tool_index as i32));
        body.instruction(&Instruction::I32Eq);
        body.instruction(&Instruction::If(BlockType::Empty));
        emit_agent_invoke(
            body,
            tool_invoke,
            tool_capability,
            static_data,
            tool.agent_id,
            DIRECT_AI_TOOL_ARGS_PTR_LOCAL,
            DIRECT_AI_TOOL_ARGS_LEN_LOCAL,
        );
        emit_agent_invoke_error_branch(
            body,
            indices,
            static_data,
            track_events,
            tool.agent_id,
            step_id,
            output_ptr_local,
            output_len_local,
            source_ptr_local,
            source_len_local,
            steps_ptr_local,
            steps_len_local,
            None,
            route_ptr_local,
            route_len_local,
            variables,
            data_ptr_local,
            data_len_local,
            workflow_log_kind,
            workflow_error_kind,
            None,
            None,
        );
        load_agent_retptr_list(
            body,
            DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
            DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
        );
        body.instruction(&Instruction::End);
    }

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

    body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_IDX_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_AI_TOOL_IDX_LOCAL));
    body.instruction(&Instruction::Br(0)); // continue $tool_iter
    body.instruction(&Instruction::End); // $tool_iter
    body.instruction(&Instruction::End); // $tools_done

    body.instruction(&Instruction::Br(0)); // continue $turn
    body.instruction(&Instruction::End); // $turn
    body.instruction(&Instruction::End); // $outer

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
