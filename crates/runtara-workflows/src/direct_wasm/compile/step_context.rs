// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared lowering for pure JSON steps that return an updated steps context.
//!
//! Filter, value-`Switch`, and `GroupBy` differ only in which stdlib function they
//! call; everything around them is identical — breakpoint/debug events, retptr
//! handling, folding the returned steps-context back into a fresh source (so
//! downstream `steps.X` references see the result), and tail-recursing into
//! `next_plan`. `emit_step_context_plan` is that one parameterized template,
//! collapsing three step types into a single lowering ("thin module" applied at
//! the lowering level).

use wasm_encoder::{Function as WasmFunction, Instruction};

use super::abi::{emit_retptr_error_or_return, load_retptr_list, push_retptr_arg};
use super::debug::{emit_step_breakpoint, emit_step_debug_event};
use super::dispatcher::emit_run_plan_mapping;
use super::mapping::emit_build_source;
use super::{
    DirectCoreFunctionIndices, DirectCoreStaticData, DirectDataSegment, DirectFailureTarget,
    DirectHandledTarget, DirectRunPlan, DirectVariables,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_step_context_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    step_function_index: u32,
    step_config_id: u32,
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
    body.instruction(&Instruction::I32Const(step_config_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(step_function_index));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(body, steps_ptr_local, steps_len_local);
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
