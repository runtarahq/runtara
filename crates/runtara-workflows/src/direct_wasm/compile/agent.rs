// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agent step lowering for the direct workflow core Wasm emitter.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    emit_retptr_error_or_return, load_agent_retptr_list, load_retptr_list, load_retptr_tag,
    push_retptr_arg,
};
use super::agent_error::{
    emit_agent_error_route_or_fail, emit_agent_invoke_error_body_from_info,
    emit_agent_invoke_error_branch,
};
use super::agent_invoke::emit_agent_invoke;
use super::agent_io::{emit_agent_cache_key, emit_agent_connection_input};
use super::agent_retry::{
    emit_agent_advance_retry_attempt, emit_agent_capture_retry_sleep,
    emit_agent_record_retry_attempt, emit_agent_retry_condition, emit_agent_retry_delay,
    emit_agent_retry_error_info, emit_agent_retry_sleep,
};
use super::checkpoint::{emit_checkpoint_lookup, emit_checkpoint_save};
use super::debug::{emit_agent_debug_error, emit_step_debug_event};
use super::dispatcher::emit_run_plan_mapping;
use super::mapping::{emit_apply_mapping, emit_build_source};
use super::{
    DIRECT_AGENT_RETRY_ATTEMPT_LOCAL, DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL,
    DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL, DirectCoreFunctionIndices, DirectCoreStaticData,
    DirectDataSegment, DirectErrorRoutePlan, DirectFailureTarget, DirectRunPlan, DirectVariables,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_agent_plan(
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
    max_retries: u32,
    retry_delay_ms: u64,
    rate_limit_budget_ms: u64,
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
) {
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

    emit_apply_mapping(
        body,
        indices,
        input_mapping_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        failure_target,
    );

    emit_agent_input_validation(
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
        failure_target,
    );

    emit_agent_connection_input(
        body,
        indices,
        static_data,
        agent_id,
        output_ptr_local,
        output_len_local,
    );

    if durable_checkpoint {
        emit_agent_cache_key(
            body,
            indices,
            agent_id,
            source_ptr_local,
            source_len_local,
            route_ptr_local,
            route_len_local,
        );
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

    let invoke = indices
        .agent_invokes
        .get(agent_component_id)
        .expect("direct Agent run plans have matching component imports");
    let capability_id = static_data
        .agent_capability_id(agent_id)
        .expect("direct Agent run plans have static capability ids");
    if max_retries > 0 {
        body.instruction(&Instruction::I32Const(1));
        body.instruction(&Instruction::LocalSet(DIRECT_AGENT_RETRY_ATTEMPT_LOCAL));
        body.instruction(&Instruction::Block(BlockType::Empty));
        body.instruction(&Instruction::Loop(BlockType::Empty));
        emit_agent_invoke(
            body,
            invoke,
            capability_id,
            static_data,
            agent_id,
            output_ptr_local,
            output_len_local,
        );
        load_retptr_tag(body);
        body.instruction(&Instruction::If(BlockType::Empty));
        emit_agent_capture_retry_sleep(body);
        emit_agent_retry_error_info(
            body,
            indices,
            DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL,
            DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL,
        );
        emit_agent_retry_condition(body, max_retries, retry_delay_ms, rate_limit_budget_ms);
        body.instruction(&Instruction::If(BlockType::Empty));
        emit_agent_advance_retry_attempt(body);
        emit_agent_retry_delay(
            body,
            indices,
            max_retries,
            retry_delay_ms,
            rate_limit_budget_ms,
        );
        emit_agent_retry_sleep(
            body,
            indices,
            static_data,
            durable_checkpoint,
            route_ptr_local,
            route_len_local,
            DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL,
            DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL,
        );
        if durable_checkpoint {
            emit_agent_record_retry_attempt(
                body,
                indices,
                route_ptr_local,
                route_len_local,
                DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL,
                DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL,
            );
        }
        body.instruction(&Instruction::Br(2));
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
            failure_target.map(|target| target.nested(3)),
        );
        body.instruction(&Instruction::End);
        load_agent_retptr_list(body, output_ptr_local, output_len_local);
        body.instruction(&Instruction::Br(1));
        body.instruction(&Instruction::End);
        body.instruction(&Instruction::End);
    } else {
        emit_agent_invoke(
            body,
            invoke,
            capability_id,
            static_data,
            agent_id,
            output_ptr_local,
            output_len_local,
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
            failure_target,
        );
        load_agent_retptr_list(body, output_ptr_local, output_len_local);
    }

    if durable_checkpoint {
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

    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_output));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        route_ptr_local,
        route_len_local,
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

#[allow(clippy::too_many_arguments)]
fn emit_agent_input_validation(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    agent_id: u32,
    step_id: &str,
    input_ptr_local: u32,
    input_len_local: u32,
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
    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(input_ptr_local));
    body.instruction(&Instruction::LocalGet(input_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_validate_input));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(body, route_ptr_local, route_len_local);

    body.instruction(&Instruction::LocalGet(route_len_local));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Ne);
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_agent_debug_error(
        body,
        indices,
        static_data,
        track_events,
        agent_id,
        route_ptr_local,
        route_len_local,
        input_ptr_local,
        input_len_local,
    );
    body.instruction(&Instruction::LocalGet(route_ptr_local));
    body.instruction(&Instruction::LocalSet(input_ptr_local));
    body.instruction(&Instruction::LocalGet(route_len_local));
    body.instruction(&Instruction::LocalSet(input_len_local));
    emit_agent_error_route_or_fail(
        body,
        indices,
        static_data,
        track_events,
        variables,
        step_id,
        input_ptr_local,
        input_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        input_ptr_local,
        input_len_local,
        route_ptr_local,
        route_len_local,
        error_plan,
        data_ptr_local,
        data_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target.map(|target| target.nested(1)),
    );
    body.instruction(&Instruction::End);
}
