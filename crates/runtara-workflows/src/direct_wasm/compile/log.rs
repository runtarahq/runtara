// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Log step lowering for the direct workflow core Wasm emitter.
//!
//! The simplest lowerer: emit the log payload to the host as a `workflow_log`
//! custom event (the same event stream the runtime/UI consume), then build the
//! step's `steps.X` output so the rest of the graph can reference it, and continue.
//! Logging is observability, not control flow, so there is no retry/onError
//! machinery.

use wasm_encoder::{Function as WasmFunction, Instruction};

use super::abi::{
    emit_retptr_error_or_start_step_fail, load_retptr_list, push_retptr_arg, push_segment_args,
    return_if_retptr_error,
};
use super::debug::emit_step_breakpoint;
use super::dispatcher::emit_run_plan_mapping;
use super::mapping::emit_build_source;
use super::{
    DirectCoreFunctionIndices, DirectCoreStaticData, DirectDataSegment, DirectFailureTarget,
    DirectHandledTarget, DirectRunPlan, DirectVariables,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_log_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    log_id: u32,
    breakpoint: bool,
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

    body.instruction(&Instruction::I32Const(log_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_log_event));
    // A Log emits no step-debug events on the happy path, but an unresolvable log
    // payload (e.g. a template error) should be attributed to the step and fail,
    // not silently exit via `return_if_retptr_error`. Emit a start + error pair on
    // the failure branch so the failed Log appears in the step summary.
    emit_retptr_error_or_start_step_fail(
        body,
        indices,
        static_data,
        track_events,
        failure_target,
        step_id,
        source_ptr_local,
        source_len_local,
        route_ptr_local,
        route_len_local,
        output_ptr_local,
        output_len_local,
    );
    load_retptr_list(body, output_ptr_local, output_len_local);

    push_segment_args(body, workflow_log_kind);
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));
    return_if_retptr_error(body, indices);

    body.instruction(&Instruction::I32Const(log_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_log));
    return_if_retptr_error(body, indices);
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
