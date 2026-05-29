// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agent error payload and onError route lowering for the direct core emitter.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction, MemArg};

use super::abi::{
    load_retptr_list, load_retptr_tag, push_retptr_arg, push_retptr_i32_load, push_retptr_i64_load,
    push_retptr_u8_load, push_segment_args, return_if_retptr_error,
};
use super::debug::emit_agent_debug_error;
use super::dispatcher::emit_run_plan_mapping;
use super::mapping::emit_build_source;
use super::split::emit_split_append_error_payload_and_continue;
use super::{
    DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_LEN_OFFSET, DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_PTR_OFFSET,
    DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_TAG_OFFSET, DIRECT_AGENT_RESULT_ERR_CATEGORY_LEN_OFFSET,
    DIRECT_AGENT_RESULT_ERR_CATEGORY_PTR_OFFSET, DIRECT_AGENT_RESULT_ERR_CODE_LEN_OFFSET,
    DIRECT_AGENT_RESULT_ERR_CODE_PTR_OFFSET, DIRECT_AGENT_RESULT_ERR_MESSAGE_LEN_OFFSET,
    DIRECT_AGENT_RESULT_ERR_MESSAGE_PTR_OFFSET, DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_TAG_OFFSET,
    DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_VALUE_OFFSET, DIRECT_AGENT_RESULT_ERR_RETRYABLE_OFFSET,
    DIRECT_AGENT_RESULT_ERR_SEVERITY_LEN_OFFSET, DIRECT_AGENT_RESULT_ERR_SEVERITY_PTR_OFFSET,
    DIRECT_RUN_RETPTR_OFFSET, DirectCoreFunctionIndices, DirectCoreStaticData, DirectDataSegment,
    DirectEdgeConditionPlan, DirectErrorRoutePlan, DirectFailureTarget, DirectRunPlan,
    DirectVariables, emit_runtime_fail_return,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_agent_invoke_error_branch(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    agent_id: u32,
    step_id: &str,
    output_ptr_local: u32,
    output_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    error_plan: Option<&DirectErrorRoutePlan>,
    route_ptr_local: u32,
    route_len_local: u32,
    variables: DirectVariables<'_>,
    data_ptr_local: u32,
    data_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    load_retptr_tag(body);
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_agent_invoke_error_body(
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
        failure_target.map(|target| target.nested(1)),
    );
    body.instruction(&Instruction::End);
}

#[allow(clippy::too_many_arguments)]
fn emit_agent_invoke_error_body(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    agent_id: u32,
    step_id: &str,
    output_ptr_local: u32,
    output_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    error_plan: Option<&DirectErrorRoutePlan>,
    route_ptr_local: u32,
    route_len_local: u32,
    variables: DirectVariables<'_>,
    data_ptr_local: u32,
    data_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    emit_agent_error(body, indices, agent_id, output_ptr_local, output_len_local);
    emit_agent_debug_error(
        body,
        indices,
        static_data,
        track_events,
        agent_id,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
    );
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
        failure_target,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_agent_invoke_error_body_from_info(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    agent_id: u32,
    step_id: &str,
    output_ptr_local: u32,
    output_len_local: u32,
    error_info_ptr_local: u32,
    error_info_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    error_plan: Option<&DirectErrorRoutePlan>,
    route_ptr_local: u32,
    route_len_local: u32,
    variables: DirectVariables<'_>,
    data_ptr_local: u32,
    data_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    emit_agent_error_from_info(
        body,
        indices,
        agent_id,
        error_info_ptr_local,
        error_info_len_local,
        output_ptr_local,
        output_len_local,
    );
    emit_agent_debug_error(
        body,
        indices,
        static_data,
        track_events,
        agent_id,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
    );
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
        failure_target,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_agent_error_route_or_fail(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    error_ptr_local: u32,
    error_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
    error_plan: Option<&DirectErrorRoutePlan>,
    data_ptr_local: u32,
    data_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
) {
    if let Some(error_plan) = error_plan {
        emit_error_steps(
            body,
            indices,
            static_data,
            step_id,
            error_ptr_local,
            error_len_local,
            steps_ptr_local,
            steps_len_local,
        );
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
        emit_error_route_dispatch(
            body,
            indices,
            static_data,
            track_events,
            variables,
            error_plan,
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
        );
    }

    if let Some(failure_target) = failure_target {
        emit_split_append_error_payload_and_continue(
            body,
            indices,
            failure_target,
            error_ptr_local,
            error_len_local,
        );
    } else {
        emit_runtime_fail_return(body, indices, error_ptr_local, error_len_local);
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_error_steps(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    step_id: &str,
    error_ptr_local: u32,
    error_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
) {
    let step_id = static_data
        .step_id(step_id)
        .expect("run plan step ids are present in static data");
    push_segment_args(body, step_id);
    body.instruction(&Instruction::LocalGet(error_ptr_local));
    body.instruction(&Instruction::LocalGet(error_len_local));
    body.instruction(&Instruction::LocalGet(steps_ptr_local));
    body.instruction(&Instruction::LocalGet(steps_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_error_steps));
    return_if_retptr_error(body);
    load_retptr_list(body, steps_ptr_local, steps_len_local);
}

#[allow(clippy::too_many_arguments)]
fn emit_error_route_dispatch(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    error_plan: &DirectErrorRoutePlan,
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
    emit_error_route_dispatch_inner(
        body,
        indices,
        static_data,
        track_events,
        variables,
        &error_plan.branches,
        error_plan.default_plan.as_deref(),
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
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_error_route_dispatch_inner(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    branches: &[DirectEdgeConditionPlan],
    default_plan: Option<&DirectRunPlan>,
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
    let Some((branch, remaining)) = branches.split_first() else {
        if let Some(default_plan) = default_plan {
            emit_terminal_run_plan_mapping(
                body,
                indices,
                static_data,
                track_events,
                variables,
                default_plan,
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
            );
        }
        return;
    };

    body.instruction(&Instruction::I32Const(branch.condition_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_eval_condition));
    return_if_retptr_error(body);

    body.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    body.instruction(&Instruction::I32Load8U(MemArg {
        offset: 4,
        align: 0,
        memory_index: 0,
    }));
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_terminal_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        variables,
        &branch.plan,
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
    );
    body.instruction(&Instruction::Else);
    emit_error_route_dispatch_inner(
        body,
        indices,
        static_data,
        track_events,
        variables,
        remaining,
        default_plan,
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
    );
    body.instruction(&Instruction::End);
}

#[allow(clippy::too_many_arguments)]
fn emit_terminal_run_plan_mapping(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    run_plan: &DirectRunPlan,
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
    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        variables,
        run_plan,
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
    );

    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_complete));
    load_retptr_tag(body);
    body.instruction(&Instruction::Return);
}

fn emit_agent_error(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    agent_id: u32,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    body.instruction(&Instruction::I32Const(agent_id as i32));
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_CODE_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_CODE_LEN_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_MESSAGE_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_MESSAGE_LEN_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_CATEGORY_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_CATEGORY_LEN_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_SEVERITY_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_SEVERITY_LEN_OFFSET);
    push_retptr_u8_load(body, DIRECT_AGENT_RESULT_ERR_RETRYABLE_OFFSET);
    push_retptr_u8_load(body, DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_TAG_OFFSET);
    push_retptr_i64_load(body, DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_VALUE_OFFSET);
    push_retptr_u8_load(body, DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_TAG_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_LEN_OFFSET);
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_error));
    return_if_retptr_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);
}

fn emit_agent_error_from_info(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    agent_id: u32,
    error_info_ptr_local: u32,
    error_info_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(error_info_ptr_local));
    body.instruction(&Instruction::LocalGet(error_info_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_error_from_info));
    return_if_retptr_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);
}
