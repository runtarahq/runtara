// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! WaitForSignal and onWait lowering for the direct workflow core emitter.
//!
//! Suspends the workflow until an external/human signal arrives. Because the wait
//! must survive suspension/resume, the signal id is derived deterministically
//! (instance + step + source, stable across replays) and the wait is a durable
//! poll loop — poll the named signal, heartbeat, check lifecycle signals
//! (early-return to suspend), enforce an optional timeout — rather than a host
//! blocking wait. An optional `onWait` subgraph runs first; its lowering
//! saves/restores the outer wait's shared locals on the operand stack so a wait
//! nested inside another wait's onWait branch stays correct. `emit_ai_wait_tool_arm`
//! is the AiAgent human-in-the-loop tool variant.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    emit_entry_suspend_on_signal, emit_retptr_error_or_return, load_retptr_list,
    load_retptr_option_list, push_i64_load_from_ptr, push_retptr_arg, push_retptr_i64_load,
    push_retptr_u8_load, push_segment_args, return_if_retptr_error, store_local_i64_at,
};
use super::agent_error::emit_agent_error_route_or_fail;
use super::checkpoint::{emit_checkpoint_lookup, emit_checkpoint_save};
use super::debug::{emit_step_breakpoint, emit_step_debug_event, emit_wait_debug_start_event};
use super::dispatcher::emit_run_plan_mapping;
use super::mapping::emit_build_source;
use super::split::emit_split_append_error_payload_and_continue;
use super::{
    DIRECT_RESULT_OPTION_TAG_OFFSET, DIRECT_RESULT_OPTION_U64_TAG_OFFSET,
    DIRECT_RESULT_OPTION_U64_VALUE_OFFSET, DIRECT_RET_BOOL_OK_OFFSET, DIRECT_RET_U64_OK_OFFSET,
    DIRECT_STEP_ERROR_LEN_LOCAL, DIRECT_STEP_ERROR_PTR_LOCAL, DIRECT_WAIT_DEADLINE_MS_LOCAL,
    DIRECT_WAIT_DEADLINE_SCRATCH_OFFSET, DIRECT_WAIT_ON_WAIT_VARIABLES_LEN_LOCAL,
    DIRECT_WAIT_ON_WAIT_VARIABLES_PTR_LOCAL, DIRECT_WAIT_PARENT_STEPS_LEN_LOCAL,
    DIRECT_WAIT_PARENT_STEPS_PTR_LOCAL, DIRECT_WAIT_POLL_INTERVAL_MS_LOCAL,
    DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL, DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL, DIRECT_WAIT_TIMEOUT_MS_LOCAL,
    DIRECT_WAIT_TIMEOUT_PRESENT_LOCAL, DirectCoreFunctionIndices, DirectCoreStaticData,
    DirectDataSegment, DirectErrorRoutePlan, DirectFailureTarget, DirectHandledTarget,
    DirectRunPlan, DirectVariables, emit_runtime_fail_return,
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
    return_if_retptr_error(body, indices);
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
    return_if_retptr_error(body, indices);
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
    return_if_retptr_error(body, indices);
    load_retptr_list(body, output_ptr_local, output_len_local);
    push_segment_args(body, &static_data.external_input_requested_kind);
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));
    return_if_retptr_error(body, indices);

    // poll_interval = wait-poll-interval-ms(wait_step)
    push_segment_args(body, wait_step_segment);
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_wait_poll_interval_ms));
    return_if_retptr_error(body, indices);
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_POLL_INTERVAL_MS_LOCAL));

    // Durable poll loop: exit when the signal arrives.
    body.instruction(&Instruction::Block(BlockType::Empty));
    body.instruction(&Instruction::Loop(BlockType::Empty));

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_check_signals));
    return_if_retptr_error(body, indices);
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    // Suspend-and-exit: ABI-aware (clean-run tag vs suspended outcome).
    super::abi::emit_entry_suspend_return(body, indices);
    body.instruction(&Instruction::End);

    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_poll_custom_signal));
    return_if_retptr_error(body, indices);
    push_retptr_u8_load(body, DIRECT_RESULT_OPTION_TAG_OFFSET);
    body.instruction(&Instruction::BrIf(1));

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_heartbeat));
    return_if_retptr_error(body, indices);

    // Store-freeing (gated; invoke export only): a human-in-the-loop AI tool
    // wait has no timeout, so it suspends on-signal with NO deadline — the
    // custom-signal waker is the sole wake path. Default stays the blocking
    // poll loop (byte-preserved).
    let store_freeing = indices.store_freeing_sleep
        && indices.abi == crate::direct_wasm::component::WorkflowAbi::InvokeHostImports;
    if store_freeing {
        emit_entry_suspend_on_signal(
            body,
            DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL,
            DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL,
            None,
        );
    } else {
        body.instruction(&Instruction::LocalGet(DIRECT_WAIT_POLL_INTERVAL_MS_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.runtime_blocking_sleep));
        return_if_retptr_error(body, indices);

        body.instruction(&Instruction::Br(0));
    }
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);

    // The received payload is the retptr Option's Some value.
    load_retptr_option_list(body, output_ptr_local, output_len_local);

    // tool_result = ai-wait-tool-result(payload)
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_ai_wait_tool_result));
    return_if_retptr_error(body, indices);
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
    return_if_retptr_error(body, indices);
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
    return_if_retptr_error(body, indices);
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
    // Persist the absolute deadline so a drained/resumed wait fires at the
    // ORIGINAL deadline instead of recomputing `now + timeout` on every
    // replay-from-start (which slides the deadline forward and it never fires).
    // The deadline is checkpointed under the wait's deterministic signal id
    // (checkpoints table — a distinct keyspace from the pending-signal row that
    // carries the payload). On the first entry (cache miss) we compute and save
    // it; on every resume the lookup hits and we read the stored value back.
    emit_checkpoint_lookup(
        body,
        indices,
        DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL,
        DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL,
        output_ptr_local,
        output_len_local,
    );
    push_i64_load_from_ptr(body, output_ptr_local);
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_DEADLINE_MS_LOCAL));
    body.instruction(&Instruction::Else);
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_now_ms));
    return_if_retptr_error(body, indices);
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_TIMEOUT_MS_LOCAL));
    body.instruction(&Instruction::I64Add);
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_DEADLINE_MS_LOCAL));
    store_local_i64_at(
        body,
        DIRECT_WAIT_DEADLINE_SCRATCH_OFFSET,
        DIRECT_WAIT_DEADLINE_MS_LOCAL,
    );
    body.instruction(&Instruction::I32Const(DIRECT_WAIT_DEADLINE_SCRATCH_OFFSET));
    body.instruction(&Instruction::LocalSet(output_ptr_local));
    body.instruction(&Instruction::I32Const(8));
    body.instruction(&Instruction::LocalSet(output_len_local));
    emit_checkpoint_save(
        body,
        indices,
        DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL,
        DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL,
        output_ptr_local,
        output_len_local,
    );
    body.instruction(&Instruction::End);
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
    return_if_retptr_error(body, indices);

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
    return_if_retptr_error(body, indices);
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    // Suspend-and-exit: ABI-aware (clean-run tag vs suspended outcome).
    super::abi::emit_entry_suspend_return(body, indices);
    body.instruction(&Instruction::End);

    body.instruction(&Instruction::LocalGet(route_ptr_local));
    body.instruction(&Instruction::LocalGet(route_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_poll_custom_signal));
    return_if_retptr_error(body, indices);
    push_retptr_u8_load(body, DIRECT_RESULT_OPTION_TAG_OFFSET);
    body.instruction(&Instruction::BrIf(1));

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_heartbeat));
    return_if_retptr_error(body, indices);

    emit_wait_timeout_check(
        body,
        indices,
        static_data,
        track_events,
        variables,
        step_id,
        step_id_segment,
        route_ptr_local,
        route_len_local,
        output_ptr_local,
        output_len_local,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        workflow_log_kind,
        workflow_error_kind,
        error_plan,
        failure_target,
        handled_target,
    );

    // Store-freeing Wait (gated; invoke export only): after one poll MISS and
    // the timeout check, EXIT with `suspended(on-signal{signal-id, deadline})`
    // instead of blocking the Store for the poll interval. The host parks the
    // instance (sleep_until = timeout deadline, or NULL when there is none) and
    // the custom-signal waker relaunches it when the signal arrives; the replay
    // re-polls the now-present signal and continues. Default stays the blocking
    // poll loop — byte-preserved.
    let store_freeing = indices.store_freeing_sleep
        && indices.abi == crate::direct_wasm::component::WorkflowAbi::InvokeHostImports;
    if store_freeing {
        emit_entry_suspend_on_signal(
            body,
            DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL,
            DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL,
            Some((
                DIRECT_WAIT_TIMEOUT_PRESENT_LOCAL,
                DIRECT_WAIT_DEADLINE_MS_LOCAL,
            )),
        );
    } else {
        body.instruction(&Instruction::LocalGet(DIRECT_WAIT_POLL_INTERVAL_MS_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.runtime_blocking_sleep));
        return_if_retptr_error(body, indices);

        body.instruction(&Instruction::Br(0));
    }
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
    // Save the outer wait's signal id / deadline / timeout onto the operand stack
    // around the onWait subgraph: if it contains a nested WaitForSignal, that
    // nested wait reuses these shared locals, so they must be restored before the
    // outer poll resumes. The save is LIFO (nesting-safe), survives the onWait's
    // handled `br` (it targets the Block, leaving these beneath untouched), and is
    // abandoned harmlessly on the onWait failure path (which returns).
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_TIMEOUT_PRESENT_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_DEADLINE_MS_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_TIMEOUT_MS_LOCAL));
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
    // Restore the outer wait's signal id / deadline / timeout (reverse push order)
    // before the route/steps restore below re-derives `route` from the signal id.
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_TIMEOUT_MS_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_DEADLINE_MS_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_TIMEOUT_PRESENT_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL));

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
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    step_id_segment: &DirectDataSegment,
    signal_id_ptr_local: u32,
    signal_id_len_local: u32,
    error_ptr_local: u32,
    error_len_local: u32,
    data_ptr_local: u32,
    data_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    error_plan: Option<&DirectErrorRoutePlan>,
    failure_target: Option<DirectFailureTarget>,
    handled_target: Option<DirectHandledTarget>,
) {
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_TIMEOUT_PRESENT_LOCAL));
    body.instruction(&Instruction::If(BlockType::Empty));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_now_ms));
    return_if_retptr_error(body, indices);
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_DEADLINE_MS_LOCAL));
    body.instruction(&Instruction::I64GeU);
    body.instruction(&Instruction::If(BlockType::Empty));
    push_segment_args(body, step_id_segment);
    body.instruction(&Instruction::LocalGet(signal_id_ptr_local));
    body.instruction(&Instruction::LocalGet(signal_id_len_local));
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_TIMEOUT_MS_LOCAL));
    push_retptr_arg(body);
    // Routed handlers get the structured envelope (steps.__error.code etc.);
    // the plain-string message stays the /failed payload for parity.
    if error_plan.is_some() {
        body.instruction(&Instruction::Call(
            indices.stdlib_wait_timeout_error_envelope,
        ));
    } else {
        body.instruction(&Instruction::Call(indices.stdlib_wait_timeout_error));
    }
    return_if_retptr_error(body, indices);
    load_retptr_list(body, error_ptr_local, error_len_local);
    if error_plan.is_some() {
        // GAP-14: route the WAIT_TIMEOUT error to the step's onError handler.
        // The steps context is still the parent's at this point (the wait has
        // stored nothing yet), so the error routes against it directly. The
        // signal id (route scratch) is dead on this terminal path; the error
        // is stashed in the shared step-error locals so the route dispatch
        // can use the error/output pairs as scratch. The timeout site sits 4
        // blocks deep ($outer/$poll plus the two timeout Ifs), so rejoining
        // handlers and split collectors nest by 4.
        body.instruction(&Instruction::LocalGet(error_ptr_local));
        body.instruction(&Instruction::LocalSet(DIRECT_STEP_ERROR_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(error_len_local));
        body.instruction(&Instruction::LocalSet(DIRECT_STEP_ERROR_LEN_LOCAL));
        emit_agent_error_route_or_fail(
            body,
            indices,
            static_data,
            track_events,
            variables,
            step_id,
            DIRECT_STEP_ERROR_PTR_LOCAL,
            DIRECT_STEP_ERROR_LEN_LOCAL,
            steps_ptr_local,
            steps_len_local,
            source_ptr_local,
            source_len_local,
            error_ptr_local,
            error_len_local,
            signal_id_ptr_local,
            signal_id_len_local,
            error_plan,
            data_ptr_local,
            data_len_local,
            workflow_log_kind,
            workflow_error_kind,
            failure_target.map(|target| target.nested(4)),
            handled_target.map(|target| target.nested(4)),
        );
    } else if let Some(failure_target) = failure_target {
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
