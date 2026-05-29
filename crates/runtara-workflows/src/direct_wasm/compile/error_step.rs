// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Terminal Error step lowering for the direct workflow core Wasm emitter.

use wasm_encoder::{Function as WasmFunction, Instruction};

use super::abi::{load_retptr_list, push_retptr_arg, push_segment_args, return_if_retptr_error};
use super::debug::emit_step_debug_event;
use super::{
    DirectCoreFunctionIndices, DirectCoreStaticData, DirectDataSegment, DirectFailureTarget,
    emit_split_append_error_payload_and_continue,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_error_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    step_id: &str,
    error_id: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
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
    body.instruction(&Instruction::I32Const(error_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_error_event));
    return_if_retptr_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);

    push_segment_args(body, workflow_error_kind);
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));
    return_if_retptr_error(body);

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

    body.instruction(&Instruction::I32Const(error_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_error));
    return_if_retptr_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);

    if let Some(failure_target) = failure_target {
        emit_split_append_error_payload_and_continue(
            body,
            indices,
            failure_target,
            output_ptr_local,
            output_len_local,
        );
    } else {
        body.instruction(&Instruction::LocalGet(output_ptr_local));
        body.instruction(&Instruction::LocalGet(output_len_local));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.runtime_fail));
        body.instruction(&Instruction::I32Const(1));
        body.instruction(&Instruction::Return);
    }
}
