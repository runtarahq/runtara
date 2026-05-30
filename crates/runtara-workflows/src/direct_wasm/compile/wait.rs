// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! WaitForSignal and onWait lowering for the direct workflow core emitter.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    emit_retptr_error_or_return, load_retptr_list, load_retptr_option_list, push_retptr_arg,
    push_retptr_i64_load, push_retptr_u8_load, push_segment_args, return_if_retptr_error,
};
use super::debug::{emit_step_breakpoint, emit_step_debug_event, emit_wait_debug_start_event};
use super::dispatcher::emit_run_plan_mapping;
use super::mapping::emit_build_source;
use super::split::emit_split_append_error_payload_and_continue;
use super::{
    DIRECT_RESULT_OPTION_TAG_OFFSET, DIRECT_RESULT_OPTION_U64_TAG_OFFSET,
    DIRECT_RESULT_OPTION_U64_VALUE_OFFSET, DIRECT_RET_BOOL_OK_OFFSET, DIRECT_RET_U64_OK_OFFSET,
    DIRECT_WAIT_DEADLINE_MS_LOCAL, DIRECT_WAIT_ON_WAIT_VARIABLES_LEN_LOCAL,
    DIRECT_WAIT_ON_WAIT_VARIABLES_PTR_LOCAL, DIRECT_WAIT_PARENT_STEPS_LEN_LOCAL,
    DIRECT_WAIT_PARENT_STEPS_PTR_LOCAL, DIRECT_WAIT_POLL_INTERVAL_MS_LOCAL,
    DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL, DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL, DIRECT_WAIT_TIMEOUT_MS_LOCAL,
    DIRECT_WAIT_TIMEOUT_PRESENT_LOCAL, DirectCoreFunctionIndices, DirectCoreStaticData,
    DirectDataSegment, DirectFailureTarget, DirectHandledTarget, DirectRunPlan, DirectVariables,
    emit_runtime_fail_return,
};

/// Lower a WaitForSignal target used as an AiAgent tool: emit the external-input
/// request, durably poll for the human signal, and leave the wrapped payload in
/// the tool-result locals. Mirrors the generated `emit_wait_for_signal_tool_arm`
/// — invoked from the AiAgent loop's tool dispatch (no next-plan continuation).
/// `ai_step_id` is the AiAgent step (the signal path component), `wait_step_id`
/// the WaitForSignal config owner, `label` the advertised tool name, and
/// `tool_call_counter_local` the monotonic per-call counter for the signal id.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_ai_wait_tool_arm(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    ai_step_id: &str,
    wait_step_id: &str,
    label: &str,
    tool_call_counter_local: u32,
    tool_result_ptr_local: u32,
    tool_result_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    let ai_step_segment = static_data
        .step_id(ai_step_id)
        .expect("AiAgent step id is present in static data");
    let wait_step_segment = static_data
        .step_id(wait_step_id)
        .expect("WaitForSignal tool step id is present in static data");
    let label_segment = static_data
        .step_id(label)
        .expect("AiAgent tool label is interned in static data");

    // instance_id = runtime.instance-id()
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_instance_id));
    return_if_retptr_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);

    // signal_id = ai-wait-tool-signal-id(ai_step, instance, label, counter, source)
    push_segment_args(body, ai_step_segment);
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_segment_args(body, label_segment);
    body.instruction(&Instruction::LocalGet(tool_call_counter_local));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_ai_wait_tool_signal_id));
    return_if_retptr_error(body);
    load_retptr_list(
        body,
        DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL,
        DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL,
    );

    // event = wait-event(wait_step, signal_id, source); runtime.custom-event(...)
    push_segment_args(body, wait_step_segment);
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_wait_event));
    return_if_retptr_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);
    push_segment_args(body, &static_data.external_input_requested_kind);
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));
    return_if_retptr_error(body);

    // poll_interval = wait-poll-interval-ms(wait_step)
    push_segment_args(body, wait_step_segment);
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_wait_poll_interval_ms));
    return_if_retptr_error(body);
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_POLL_INTERVAL_MS_LOCAL));

    // Durable poll loop: exit when the signal arrives.
    body.instruction(&Instruction::Block(BlockType::Empty));
    body.instruction(&Instruction::Loop(BlockType::Empty));

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_check_signals));
    return_if_retptr_error(body);
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::Return);
    body.instruction(&Instruction::End);

    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_poll_custom_signal));
    return_if_retptr_error(body);
    push_retptr_u8_load(body, DIRECT_RESULT_OPTION_TAG_OFFSET);
    body.instruction(&Instruction::BrIf(1));

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_heartbeat));
    return_if_retptr_error(body);

    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_POLL_INTERVAL_MS_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_blocking_sleep));
    return_if_retptr_error(body);

    body.instruction(&Instruction::Br(0));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);

    // The received payload is the retptr Option's Some value.
    load_retptr_option_list(body, output_ptr_local, output_len_local);

    // tool_result = ai-wait-tool-result(payload)
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_ai_wait_tool_result));
    return_if_retptr_error(body);
    load_retptr_list(body, tool_result_ptr_local, tool_result_len_local);
}

pub(super) fn emit_wait_on_wait_error_and_fail(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    target: DirectFailureTarget,
    error_ptr_local: u32,
    error_len_local: u32,
) {
    let DirectFailureTarget::WaitOnWait {
        step_id_offset,
        step_id_len,
    } = target
    else {
        unreachable!("non-onWait target passed to onWait failure emitter");
    };
    body.instruction(&Instruction::I32Const(step_id_offset));
    body.instruction(&Instruction::I32Const(step_id_len));
    body.instruction(&Instruction::LocalGet(error_ptr_local));
    body.instruction(&Instruction::LocalGet(error_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_wait_on_wait_error));
    return_if_retptr_error(body);
    load_retptr_list(body, error_ptr_local, error_len_local);
    emit_runtime_fail_return(body, indices, error_ptr_local, error_len_local);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_wait_for_signal_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    breakpoint: bool,
    on_wait_plan: Option<&DirectRunPlan>,
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
    handled_target: Option<DirectHandledTarget>,
) {
    let step_id_segment = static_data
        .step_id(step_id)
        .expect("run plan step ids are present in static data");

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

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_instance_id));
    return_if_retptr_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);

    push_segment_args(body, step_id_segment);
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_wait_signal_id));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        output_ptr_local,
        output_len_local,
    );
    load_retptr_list(body, route_ptr_local, route_len_local);
    body.instruction(&Instruction::LocalGet(route_ptr_local));
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(route_len_local));
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL));

    push_segment_args(body, step_id_segment);
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_wait_timeout_ms));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        output_ptr_local,
        output_len_local,
    );
    push_retptr_u8_load(body, DIRECT_RESULT_OPTION_U64_TAG_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_TIMEOUT_PRESENT_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_TIMEOUT_PRESENT_LOCAL));
    body.instruction(&Instruction::If(BlockType::Empty));
    push_retptr_i64_load(body, DIRECT_RESULT_OPTION_U64_VALUE_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_TIMEOUT_MS_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_now_ms));
    return_if_retptr_error(body);
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_TIMEOUT_MS_LOCAL));
    body.instruction(&Instruction::I64Add);
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_DEADLINE_MS_LOCAL));
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::I64Const(0));
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_TIMEOUT_MS_LOCAL));
    body.instruction(&Instruction::I64Const(0));
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_DEADLINE_MS_LOCAL));
    body.instruction(&Instruction::End);

    emit_wait_debug_start_event(
        body,
        indices,
        static_data,
        track_events,
        step_id,
        DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL,
        DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL,
        DIRECT_WAIT_TIMEOUT_PRESENT_LOCAL,
        DIRECT_WAIT_TIMEOUT_MS_LOCAL,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        failure_target,
    );

    if let Some(on_wait_plan) = on_wait_plan {
        emit_wait_on_wait_plan(
            body,
            indices,
            static_data,
            track_events,
            variables,
            step_id_segment,
            on_wait_plan,
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

    push_segment_args(body, step_id_segment);
    body.instruction(&Instruction::LocalGet(route_ptr_local));
    body.instruction(&Instruction::LocalGet(route_len_local));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_wait_event));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        output_ptr_local,
        output_len_local,
    );
    load_retptr_list(body, output_ptr_local, output_len_local);

    push_segment_args(body, &static_data.external_input_requested_kind);
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));
    return_if_retptr_error(body);

    push_segment_args(body, step_id_segment);
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_wait_poll_interval_ms));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        output_ptr_local,
        output_len_local,
    );
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_POLL_INTERVAL_MS_LOCAL));

    body.instruction(&Instruction::Block(BlockType::Empty));
    body.instruction(&Instruction::Loop(BlockType::Empty));

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_check_signals));
    return_if_retptr_error(body);
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::Return);
    body.instruction(&Instruction::End);

    body.instruction(&Instruction::LocalGet(route_ptr_local));
    body.instruction(&Instruction::LocalGet(route_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_poll_custom_signal));
    return_if_retptr_error(body);
    push_retptr_u8_load(body, DIRECT_RESULT_OPTION_TAG_OFFSET);
    body.instruction(&Instruction::BrIf(1));

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_heartbeat));
    return_if_retptr_error(body);

    emit_wait_timeout_check(
        body,
        indices,
        step_id_segment,
        route_ptr_local,
        route_len_local,
        output_ptr_local,
        output_len_local,
        failure_target,
    );

    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_POLL_INTERVAL_MS_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_blocking_sleep));
    return_if_retptr_error(body);

    body.instruction(&Instruction::Br(0));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);

    load_retptr_option_list(body, output_ptr_local, output_len_local);

    push_segment_args(body, step_id_segment);
    body.instruction(&Instruction::LocalGet(route_ptr_local));
    body.instruction(&Instruction::LocalGet(route_len_local));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_wait_output));
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

#[allow(clippy::too_many_arguments)]
fn emit_wait_on_wait_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    parent_variables: DirectVariables<'_>,
    step_id_segment: &DirectDataSegment,
    on_wait_plan: &DirectRunPlan,
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
    push_segment_args(body, step_id_segment);
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_wait_on_wait_variables));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(
        body,
        DIRECT_WAIT_ON_WAIT_VARIABLES_PTR_LOCAL,
        DIRECT_WAIT_ON_WAIT_VARIABLES_LEN_LOCAL,
    );

    body.instruction(&Instruction::LocalGet(steps_ptr_local));
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_PARENT_STEPS_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(steps_len_local));
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_PARENT_STEPS_LEN_LOCAL));
    body.instruction(&Instruction::I32Const(static_data.steps.offset));
    body.instruction(&Instruction::LocalSet(steps_ptr_local));
    body.instruction(&Instruction::I32Const(static_data.steps.len_i32()));
    body.instruction(&Instruction::LocalSet(steps_len_local));

    let on_wait_variables = DirectVariables::Locals {
        ptr_local: DIRECT_WAIT_ON_WAIT_VARIABLES_PTR_LOCAL,
        len_local: DIRECT_WAIT_ON_WAIT_VARIABLES_LEN_LOCAL,
    };
    let on_wait_failure_target = DirectFailureTarget::WaitOnWait {
        step_id_offset: step_id_segment.offset,
        step_id_len: step_id_segment.len_i32(),
    };
    emit_build_source(
        body,
        indices,
        on_wait_variables,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        Some(on_wait_failure_target),
    );
    body.instruction(&Instruction::Block(BlockType::Empty));
    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        on_wait_variables,
        on_wait_plan,
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
        Some(on_wait_failure_target),
        Some(DirectHandledTarget { branch_depth: 0 }),
    );
    body.instruction(&Instruction::End);

    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_PARENT_STEPS_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(steps_ptr_local));
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_PARENT_STEPS_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(steps_len_local));
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(route_ptr_local));
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(route_len_local));

    emit_build_source(
        body,
        indices,
        parent_variables,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        failure_target,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_wait_timeout_check(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    step_id_segment: &DirectDataSegment,
    signal_id_ptr_local: u32,
    signal_id_len_local: u32,
    error_ptr_local: u32,
    error_len_local: u32,
    failure_target: Option<DirectFailureTarget>,
) {
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_TIMEOUT_PRESENT_LOCAL));
    body.instruction(&Instruction::If(BlockType::Empty));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_now_ms));
    return_if_retptr_error(body);
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_DEADLINE_MS_LOCAL));
    body.instruction(&Instruction::I64GeU);
    body.instruction(&Instruction::If(BlockType::Empty));
    push_segment_args(body, step_id_segment);
    body.instruction(&Instruction::LocalGet(signal_id_ptr_local));
    body.instruction(&Instruction::LocalGet(signal_id_len_local));
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_TIMEOUT_MS_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_wait_timeout_error));
    return_if_retptr_error(body);
    load_retptr_list(body, error_ptr_local, error_len_local);
    if let Some(failure_target) = failure_target {
        emit_split_append_error_payload_and_continue(
            body,
            indices,
            failure_target.nested(4),
            error_ptr_local,
            error_len_local,
        );
    } else {
        emit_runtime_fail_return(body, indices, error_ptr_local, error_len_local);
    }
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
}
