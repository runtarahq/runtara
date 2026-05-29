// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Routing Switch step lowering for the direct workflow core Wasm emitter.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction, MemArg, ValType};

use super::abi::{emit_retptr_error_or_return, load_retptr_list, push_retptr_arg};
use super::{
    DirectCoreFunctionIndices, DirectCoreStaticData, DirectDataSegment, DirectFailureTarget,
    DirectRunPlan, DirectSwitchRoutePlan, DirectVariables, emit_build_source,
    emit_run_plan_mapping, emit_step_debug_event,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_switch_route_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    switch_id: u32,
    branches: &[DirectSwitchRoutePlan],
    default_plan: &DirectRunPlan,
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
    body.instruction(&Instruction::I32Const(switch_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_process_switch));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(body, route_ptr_local, route_len_local);

    body.instruction(&Instruction::I32Const(switch_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_value_switch));
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

    emit_switch_route_dispatch(
        body,
        indices,
        static_data,
        track_events,
        variables,
        branches,
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
        failure_target,
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_switch_route_dispatch(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    branches: &[DirectSwitchRoutePlan],
    default_plan: &DirectRunPlan,
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
    let Some((branch, remaining)) = branches.split_first() else {
        emit_run_plan_mapping(
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
            failure_target,
        );
        return;
    };

    emit_route_equals(body, route_ptr_local, route_len_local, &branch.label);
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_run_plan_mapping(
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
        failure_target.map(|target| target.nested(1)),
    );
    body.instruction(&Instruction::Else);
    emit_switch_route_dispatch(
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
        failure_target.map(|target| target.nested(1)),
    );
    body.instruction(&Instruction::End);
}

fn emit_route_equals(
    body: &mut WasmFunction,
    route_ptr_local: u32,
    route_len_local: u32,
    label: &str,
) {
    body.instruction(&Instruction::LocalGet(route_len_local));
    body.instruction(&Instruction::I32Const(label.len() as i32));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
    body.instruction(&Instruction::I32Const(1));

    for (offset, byte) in label.as_bytes().iter().enumerate() {
        body.instruction(&Instruction::LocalGet(route_ptr_local));
        body.instruction(&Instruction::I32Load8U(MemArg {
            offset: offset as u64,
            align: 0,
            memory_index: 0,
        }));
        body.instruction(&Instruction::I32Const(i32::from(*byte)));
        body.instruction(&Instruction::I32Eq);
        body.instruction(&Instruction::I32And);
    }
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::End);
}
