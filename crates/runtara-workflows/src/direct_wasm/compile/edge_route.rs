// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Conditional edge-route dispatch lowering for the direct core emitter.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction, MemArg};

use super::abi::{emit_retptr_error_or_return, push_retptr_arg};
use super::dispatcher::emit_run_plan_mapping;
use super::{
    DIRECT_RUN_RETPTR_OFFSET, DirectCoreFunctionIndices, DirectCoreStaticData, DirectDataSegment,
    DirectEdgeConditionPlan, DirectFailureTarget, DirectHandledTarget, DirectRunPlan,
    DirectVariables,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_edge_route_dispatch(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    branches: &[DirectEdgeConditionPlan],
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
    handled_target: Option<DirectHandledTarget>,
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
            handled_target,
        );
        return;
    };

    body.instruction(&Instruction::I32Const(branch.condition_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_eval_condition));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        route_ptr_local,
        route_len_local,
    );

    body.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    body.instruction(&Instruction::I32Load8U(MemArg {
        offset: 4,
        align: 0,
        memory_index: 0,
    }));
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
        handled_target.map(|target| target.nested(1)),
    );
    body.instruction(&Instruction::Else);
    emit_edge_route_dispatch(
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
        handled_target.map(|target| target.nested(1)),
    );
    body.instruction(&Instruction::End);
}
