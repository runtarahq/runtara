// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! While step lowering for the direct workflow core Wasm emitter.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    emit_retptr_error_or_return, load_retptr_list, push_retptr_arg, push_retptr_i32_load,
    push_retptr_u8_load, push_variables_args,
};
use super::debug::{emit_step_breakpoint, emit_step_debug_event};
use super::dispatcher::emit_run_plan_mapping;
use super::mapping::emit_build_source;
use super::{
    DIRECT_RET_BOOL_OK_OFFSET, DIRECT_RET_U32_OK_OFFSET, DIRECT_WHILE_INDEX_LOCAL,
    DIRECT_WHILE_MAX_ITERATIONS_LOCAL, DIRECT_WHILE_PARENT_SOURCE_LEN_LOCAL,
    DIRECT_WHILE_PARENT_SOURCE_PTR_LOCAL, DIRECT_WHILE_STATE_LEN_LOCAL,
    DIRECT_WHILE_STATE_PTR_LOCAL, DIRECT_WHILE_VARIABLES_LEN_LOCAL,
    DIRECT_WHILE_VARIABLES_PTR_LOCAL, DirectCoreFunctionIndices, DirectCoreStaticData,
    DirectDataSegment, DirectFailureTarget, DirectRunPlan, DirectVariables,
};

fn push_while_frame(body: &mut WasmFunction) {
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_MAX_ITERATIONS_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_INDEX_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_VARIABLES_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_VARIABLES_LEN_LOCAL));
}

fn pop_while_frame(body: &mut WasmFunction) {
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_VARIABLES_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_VARIABLES_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_STATE_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_INDEX_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_MAX_ITERATIONS_LOCAL));
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_while_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    while_id: u32,
    breakpoint: bool,
    nested_plan: &DirectRunPlan,
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
    failure_target: Option<DirectFailureTarget>,
) {
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

    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        true,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );

    push_while_frame(body);
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(source_len_local));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_PARENT_SOURCE_LEN_LOCAL));

    body.instruction(&Instruction::I32Const(while_id as i32));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_max_iterations));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        route_ptr_local,
        route_len_local,
    );
    push_retptr_i32_load(body, DIRECT_RET_U32_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_MAX_ITERATIONS_LOCAL));

    body.instruction(&Instruction::I32Const(while_id as i32));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_initial_state));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(
        body,
        DIRECT_WHILE_STATE_PTR_LOCAL,
        DIRECT_WHILE_STATE_LEN_LOCAL,
    );

    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_INDEX_LOCAL));
    body.instruction(&Instruction::Block(BlockType::Empty));
    body.instruction(&Instruction::Loop(BlockType::Empty));
    let loop_failure_target = failure_target.map(|target| target.nested(2));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_INDEX_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_MAX_ITERATIONS_LOCAL));
    body.instruction(&Instruction::I32GeU);
    body.instruction(&Instruction::BrIf(1));

    body.instruction(&Instruction::I32Const(while_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_condition_source));
    emit_retptr_error_or_return(
        body,
        indices,
        loop_failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(body, source_ptr_local, source_len_local);

    body.instruction(&Instruction::I32Const(while_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_condition));
    emit_retptr_error_or_return(
        body,
        indices,
        loop_failure_target,
        route_ptr_local,
        route_len_local,
    );
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::BrIf(1));

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_is_cancelled));
    emit_retptr_error_or_return(
        body,
        indices,
        loop_failure_target,
        route_ptr_local,
        route_len_local,
    );
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::Return);
    body.instruction(&Instruction::End);

    body.instruction(&Instruction::I32Const(while_id as i32));
    push_variables_args(body, variables);
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_iteration_variables));
    emit_retptr_error_or_return(
        body,
        indices,
        loop_failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(
        body,
        DIRECT_WHILE_VARIABLES_PTR_LOCAL,
        DIRECT_WHILE_VARIABLES_LEN_LOCAL,
    );

    body.instruction(&Instruction::I32Const(static_data.steps.offset));
    body.instruction(&Instruction::LocalSet(steps_ptr_local));
    body.instruction(&Instruction::I32Const(static_data.steps.len_i32()));
    body.instruction(&Instruction::LocalSet(steps_len_local));

    let iteration_variables = DirectVariables::Locals {
        ptr_local: DIRECT_WHILE_VARIABLES_PTR_LOCAL,
        len_local: DIRECT_WHILE_VARIABLES_LEN_LOCAL,
    };
    emit_build_source(
        body,
        indices,
        iteration_variables,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        loop_failure_target,
    );

    push_while_frame(body);
    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        iteration_variables,
        nested_plan,
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
        loop_failure_target,
    );
    pop_while_frame(body);

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_heartbeat));
    emit_retptr_error_or_return(
        body,
        indices,
        loop_failure_target,
        route_ptr_local,
        route_len_local,
    );

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_check_signals));
    emit_retptr_error_or_return(
        body,
        indices,
        loop_failure_target,
        route_ptr_local,
        route_len_local,
    );
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::Return);
    body.instruction(&Instruction::End);

    body.instruction(&Instruction::I32Const(while_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_advance_state));
    emit_retptr_error_or_return(
        body,
        indices,
        loop_failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(
        body,
        DIRECT_WHILE_STATE_PTR_LOCAL,
        DIRECT_WHILE_STATE_LEN_LOCAL,
    );

    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_INDEX_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_INDEX_LOCAL));
    body.instruction(&Instruction::Br(0));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);

    body.instruction(&Instruction::I32Const(while_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_output));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(body, steps_ptr_local, steps_len_local);

    pop_while_frame(body);
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
        failure_target,
    );

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
        failure_target,
    );
}
