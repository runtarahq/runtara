// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! EmbedWorkflow lowering for statically composed child workflow graphs.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    emit_retptr_error_or_return, load_retptr_list, push_retptr_arg, push_retptr_u8_load,
    push_segment_args, return_if_retptr_error,
};
use super::agent_error::emit_agent_error_route_or_fail;
use super::checkpoint::{emit_checkpoint_lookup, emit_checkpoint_save};
use super::debug::{emit_step_breakpoint, emit_step_debug_event};
use super::dispatcher::emit_run_plan_mapping;
use super::embed_retry::{
    emit_embed_retry_before_attempt, emit_embed_retry_condition, emit_embed_retry_error_info,
};
use super::mapping::{emit_apply_mapping, emit_build_source};
use super::{
    DIRECT_EMBED_CHILD_DATA_LEN_LOCAL, DIRECT_EMBED_CHILD_DATA_PTR_LOCAL,
    DIRECT_EMBED_CHILD_ERROR_FLAG_LOCAL, DIRECT_EMBED_CHILD_ERROR_LEN_LOCAL,
    DIRECT_EMBED_CHILD_ERROR_PTR_LOCAL, DIRECT_EMBED_CHILD_VARIABLES_LEN_LOCAL,
    DIRECT_EMBED_CHILD_VARIABLES_PTR_LOCAL, DIRECT_EMBED_PARENT_SOURCE_LEN_LOCAL,
    DIRECT_EMBED_PARENT_SOURCE_PTR_LOCAL, DIRECT_EMBED_RATE_LIMIT_WAIT_TOTAL_LOCAL,
    DIRECT_EMBED_RETRY_ATTEMPT_LOCAL, DIRECT_EMBED_STEP_RESULT_LEN_LOCAL,
    DIRECT_EMBED_STEP_RESULT_PTR_LOCAL, DIRECT_RET_BOOL_OK_OFFSET, DirectCoreFunctionIndices,
    DirectCoreStaticData, DirectDataSegment, DirectErrorRoutePlan, DirectFailureTarget,
    DirectHandledTarget, DirectRunPlan, DirectVariables,
};

pub(super) fn emit_embed_workflow_child_error_and_continue(
    body: &mut WasmFunction,
    target: DirectFailureTarget,
    error_ptr_local: u32,
    error_len_local: u32,
) {
    let DirectFailureTarget::EmbedWorkflow { branch_depth } = target else {
        panic!("EmbedWorkflow child failure target expected");
    };

    body.instruction(&Instruction::LocalGet(error_ptr_local));
    body.instruction(&Instruction::LocalSet(DIRECT_EMBED_CHILD_ERROR_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(error_len_local));
    body.instruction(&Instruction::LocalSet(DIRECT_EMBED_CHILD_ERROR_LEN_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::LocalSet(DIRECT_EMBED_CHILD_ERROR_FLAG_LOCAL));
    body.instruction(&Instruction::Br(branch_depth));
}

fn push_embed_workflow_frame(
    body: &mut WasmFunction,
    steps_ptr_local: u32,
    steps_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
) {
    body.instruction(&Instruction::LocalGet(DIRECT_EMBED_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_EMBED_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(steps_ptr_local));
    body.instruction(&Instruction::LocalGet(steps_len_local));
    body.instruction(&Instruction::LocalGet(route_ptr_local));
    body.instruction(&Instruction::LocalGet(route_len_local));
}

fn pop_embed_workflow_frame(
    body: &mut WasmFunction,
    steps_ptr_local: u32,
    steps_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
) {
    body.instruction(&Instruction::LocalSet(route_len_local));
    body.instruction(&Instruction::LocalSet(route_ptr_local));
    body.instruction(&Instruction::LocalSet(steps_len_local));
    body.instruction(&Instruction::LocalSet(steps_ptr_local));
    body.instruction(&Instruction::LocalSet(DIRECT_EMBED_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_EMBED_PARENT_SOURCE_PTR_LOCAL));
}

fn push_embed_workflow_attempt_frame(
    body: &mut WasmFunction,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
) {
    body.instruction(&Instruction::LocalGet(
        DIRECT_EMBED_CHILD_VARIABLES_PTR_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(
        DIRECT_EMBED_CHILD_VARIABLES_LEN_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(steps_ptr_local));
    body.instruction(&Instruction::LocalGet(steps_len_local));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    body.instruction(&Instruction::LocalGet(route_ptr_local));
    body.instruction(&Instruction::LocalGet(route_len_local));
    body.instruction(&Instruction::LocalGet(DIRECT_EMBED_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::LocalGet(
        DIRECT_EMBED_RATE_LIMIT_WAIT_TOTAL_LOCAL,
    ));
}

fn pop_embed_workflow_attempt_frame(
    body: &mut WasmFunction,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
) {
    body.instruction(&Instruction::LocalSet(
        DIRECT_EMBED_RATE_LIMIT_WAIT_TOTAL_LOCAL,
    ));
    body.instruction(&Instruction::LocalSet(DIRECT_EMBED_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::LocalSet(route_len_local));
    body.instruction(&Instruction::LocalSet(route_ptr_local));
    body.instruction(&Instruction::LocalSet(source_len_local));
    body.instruction(&Instruction::LocalSet(source_ptr_local));
    body.instruction(&Instruction::LocalSet(steps_len_local));
    body.instruction(&Instruction::LocalSet(steps_ptr_local));
    body.instruction(&Instruction::LocalSet(
        DIRECT_EMBED_CHILD_VARIABLES_LEN_LOCAL,
    ));
    body.instruction(&Instruction::LocalSet(
        DIRECT_EMBED_CHILD_VARIABLES_PTR_LOCAL,
    ));
}

fn emit_wrapped_child_error(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    step_id_segment: &DirectDataSegment,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    push_segment_args(body, step_id_segment);
    body.instruction(&Instruction::LocalGet(DIRECT_EMBED_CHILD_ERROR_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_EMBED_CHILD_ERROR_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_embed_workflow_error));
    return_if_retptr_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);
}

#[allow(clippy::too_many_arguments)]
fn emit_embed_workflow_child_attempt(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    child_variables: DirectVariables<'_>,
    child_plan: &DirectRunPlan,
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
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(DIRECT_EMBED_CHILD_ERROR_FLAG_LOCAL));
    push_embed_workflow_attempt_frame(
        body,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        route_ptr_local,
        route_len_local,
    );
    body.instruction(&Instruction::Block(BlockType::Empty));
    body.instruction(&Instruction::I32Const(static_data.steps.offset));
    body.instruction(&Instruction::LocalSet(steps_ptr_local));
    body.instruction(&Instruction::I32Const(static_data.steps.len_i32()));
    body.instruction(&Instruction::LocalSet(steps_len_local));
    emit_build_source(
        body,
        indices,
        child_variables,
        DIRECT_EMBED_CHILD_DATA_PTR_LOCAL,
        DIRECT_EMBED_CHILD_DATA_LEN_LOCAL,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        Some(DirectFailureTarget::EmbedWorkflow { branch_depth: 0 }),
    );
    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        child_variables,
        child_plan,
        DIRECT_EMBED_CHILD_DATA_PTR_LOCAL,
        DIRECT_EMBED_CHILD_DATA_LEN_LOCAL,
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
        Some(DirectFailureTarget::EmbedWorkflow { branch_depth: 0 }),
        Some(DirectHandledTarget { branch_depth: 0 }),
    );
    body.instruction(&Instruction::End);
    pop_embed_workflow_attempt_frame(
        body,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        route_ptr_local,
        route_len_local,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_embed_workflow_child_with_retry(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    child_variables: DirectVariables<'_>,
    child_plan: &DirectRunPlan,
    max_retries: u32,
    retry_delay_ms: u64,
    durable: bool,
    step_id_segment: &DirectDataSegment,
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
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::LocalSet(DIRECT_EMBED_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::I64Const(0));
    body.instruction(&Instruction::LocalSet(
        DIRECT_EMBED_RATE_LIMIT_WAIT_TOTAL_LOCAL,
    ));

    if max_retries == 0 {
        emit_embed_workflow_child_attempt(
            body,
            indices,
            static_data,
            track_events,
            child_variables,
            child_plan,
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
        );
        body.instruction(&Instruction::LocalGet(DIRECT_EMBED_CHILD_ERROR_FLAG_LOCAL));
        body.instruction(&Instruction::If(BlockType::Empty));
        emit_wrapped_child_error(
            body,
            indices,
            step_id_segment,
            output_ptr_local,
            output_len_local,
        );
        body.instruction(&Instruction::End);
        return;
    }

    body.instruction(&Instruction::Block(BlockType::Empty));
    body.instruction(&Instruction::Loop(BlockType::Empty));
    emit_embed_retry_before_attempt(
        body,
        indices,
        static_data,
        durable,
        route_ptr_local,
        route_len_local,
        output_ptr_local,
        output_len_local,
        max_retries,
        retry_delay_ms,
    );
    emit_embed_workflow_child_attempt(
        body,
        indices,
        static_data,
        track_events,
        child_variables,
        child_plan,
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
    );
    body.instruction(&Instruction::LocalGet(DIRECT_EMBED_CHILD_ERROR_FLAG_LOCAL));
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_wrapped_child_error(
        body,
        indices,
        step_id_segment,
        output_ptr_local,
        output_len_local,
    );
    emit_embed_retry_error_info(body, indices, output_ptr_local, output_len_local);
    emit_embed_retry_condition(body, max_retries, retry_delay_ms);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(DIRECT_EMBED_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_EMBED_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::Br(2));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::Br(2));
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::Br(2));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_embed_workflow_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    input_mapping_id: u32,
    durable: bool,
    breakpoint: bool,
    max_retries: u32,
    retry_delay_ms: u64,
    child_plan: &DirectRunPlan,
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
    let step_id_segment = static_data
        .step_id(step_id)
        .expect("run plan step ids are present in static data");

    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalSet(DIRECT_EMBED_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(source_len_local));
    body.instruction(&Instruction::LocalSet(DIRECT_EMBED_PARENT_SOURCE_LEN_LOCAL));

    emit_apply_mapping(
        body,
        indices,
        input_mapping_id,
        DIRECT_EMBED_PARENT_SOURCE_PTR_LOCAL,
        DIRECT_EMBED_PARENT_SOURCE_LEN_LOCAL,
        DIRECT_EMBED_CHILD_DATA_PTR_LOCAL,
        DIRECT_EMBED_CHILD_DATA_LEN_LOCAL,
        failure_target,
    );

    if breakpoint {
        emit_build_source(
            body,
            indices,
            variables,
            DIRECT_EMBED_CHILD_DATA_PTR_LOCAL,
            DIRECT_EMBED_CHILD_DATA_LEN_LOCAL,
            steps_ptr_local,
            steps_len_local,
            output_ptr_local,
            output_len_local,
            failure_target,
        );
        emit_step_breakpoint(
            body,
            indices,
            static_data,
            true,
            step_id,
            output_ptr_local,
            output_len_local,
            output_ptr_local,
            output_len_local,
            route_ptr_local,
            route_len_local,
        );
    }

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

    if durable {
        push_segment_args(body, step_id_segment);
        body.instruction(&Instruction::LocalGet(DIRECT_EMBED_PARENT_SOURCE_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_EMBED_PARENT_SOURCE_LEN_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_embed_workflow_cache_key));
        return_if_retptr_error(body);
        load_retptr_list(body, route_ptr_local, route_len_local);

        emit_checkpoint_lookup(
            body,
            indices,
            route_ptr_local,
            route_len_local,
            output_ptr_local,
            output_len_local,
        );
        body.instruction(&Instruction::Else);
    }

    push_segment_args(body, step_id_segment);
    body.instruction(&Instruction::LocalGet(DIRECT_EMBED_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_EMBED_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_EMBED_CHILD_DATA_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_EMBED_CHILD_DATA_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_embed_workflow_variables));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        output_ptr_local,
        output_len_local,
    );
    load_retptr_list(
        body,
        DIRECT_EMBED_CHILD_VARIABLES_PTR_LOCAL,
        DIRECT_EMBED_CHILD_VARIABLES_LEN_LOCAL,
    );

    push_embed_workflow_frame(
        body,
        steps_ptr_local,
        steps_len_local,
        route_ptr_local,
        route_len_local,
    );

    let child_variables = DirectVariables::Locals {
        ptr_local: DIRECT_EMBED_CHILD_VARIABLES_PTR_LOCAL,
        len_local: DIRECT_EMBED_CHILD_VARIABLES_LEN_LOCAL,
    };
    emit_embed_workflow_child_with_retry(
        body,
        indices,
        static_data,
        track_events,
        child_variables,
        child_plan,
        max_retries,
        retry_delay_ms,
        durable,
        step_id_segment,
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
    );
    pop_embed_workflow_frame(
        body,
        steps_ptr_local,
        steps_len_local,
        route_ptr_local,
        route_len_local,
    );

    body.instruction(&Instruction::LocalGet(DIRECT_EMBED_CHILD_ERROR_FLAG_LOCAL));
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_agent_error_route_or_fail(
        body,
        indices,
        static_data,
        track_events,
        variables,
        step_id,
        output_ptr_local,
        output_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        error_plan,
        data_ptr_local,
        data_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target.map(|target| target.nested(1)),
        handled_target.map(|target| target.nested(1)),
    );
    body.instruction(&Instruction::End);

    push_segment_args(body, step_id_segment);
    body.instruction(&Instruction::LocalGet(DIRECT_EMBED_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_EMBED_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_embed_workflow_result));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        output_ptr_local,
        output_len_local,
    );
    load_retptr_list(
        body,
        DIRECT_EMBED_STEP_RESULT_PTR_LOCAL,
        DIRECT_EMBED_STEP_RESULT_LEN_LOCAL,
    );
    body.instruction(&Instruction::LocalGet(DIRECT_EMBED_STEP_RESULT_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(DIRECT_EMBED_STEP_RESULT_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(output_len_local));

    if durable {
        emit_checkpoint_save(
            body,
            indices,
            route_ptr_local,
            route_len_local,
            output_ptr_local,
            output_len_local,
        );
        body.instruction(&Instruction::End);
    }

    push_segment_args(body, step_id_segment);
    body.instruction(&Instruction::LocalGet(DIRECT_EMBED_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_EMBED_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(
        indices.stdlib_embed_workflow_output_from_result,
    ));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        output_ptr_local,
        output_len_local,
    );
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

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_check_signals));
    return_if_retptr_error(body);
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::Return);
    body.instruction(&Instruction::End);

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
        handled_target,
    );
}
