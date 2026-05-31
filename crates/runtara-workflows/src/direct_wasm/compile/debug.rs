// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Debug event and breakpoint lowering shared by direct core step lowerers.
//!
//! Cross-cutting observability that every step type needs identically: step
//! start/end events, breakpoint pause/resume, and agent-error events. Each helper
//! early-returns when its gate (`track_events` / `breakpoint`) is off, so a
//! non-debug build pays nothing. `emit_step_breakpoint` is checkpoint-guarded — it
//! pauses via `runtime_breakpoint_pause` and returns to suspend the instance,
//! giving breakpoints the same durable suspend/resume semantics as the generated
//! compiler. Isolating these keeps the per-step lowerers focused on semantics.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    emit_retptr_error_or_return, load_retptr_list, load_retptr_tag, push_retptr_arg,
    push_retptr_u8_load, push_segment_args, return_if_retptr_error,
};
use super::{
    DIRECT_CHECKPOINT_FOUND_OFFSET, DIRECT_RET_BOOL_OK_OFFSET, DirectCoreFunctionIndices,
    DirectCoreStaticData, DirectFailureTarget,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_step_debug_event(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    start: bool,
    step_id: &str,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    if !track_events {
        return;
    }

    let step_id = static_data
        .step_id(step_id)
        .expect("run plan step ids are present in static data");
    push_segment_args(body, step_id);
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(if start {
        indices.stdlib_step_debug_start
    } else {
        indices.stdlib_step_debug_end
    }));
    return_if_retptr_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);

    push_segment_args(
        body,
        if start {
            &static_data.step_debug_start_kind
        } else {
            &static_data.step_debug_end_kind
        },
    );
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));
    return_if_retptr_error(body);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_wait_debug_start_event(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    step_id: &str,
    signal_id_ptr_local: u32,
    signal_id_len_local: u32,
    timeout_present_local: u32,
    timeout_ms_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    failure_target: Option<DirectFailureTarget>,
) {
    if !track_events {
        return;
    }

    let step_id = static_data
        .step_id(step_id)
        .expect("run plan step ids are present in static data");
    push_segment_args(body, step_id);
    body.instruction(&Instruction::LocalGet(signal_id_ptr_local));
    body.instruction(&Instruction::LocalGet(signal_id_len_local));
    body.instruction(&Instruction::LocalGet(timeout_present_local));
    body.instruction(&Instruction::LocalGet(timeout_ms_local));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_wait_debug_start));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        output_ptr_local,
        output_len_local,
    );
    load_retptr_list(body, output_ptr_local, output_len_local);

    push_segment_args(body, &static_data.step_debug_start_kind);
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        output_ptr_local,
        output_len_local,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_step_breakpoint(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    breakpoint: bool,
    step_id: &str,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
) {
    if !breakpoint {
        return;
    }

    let step_id = static_data
        .step_id(step_id)
        .expect("run plan step ids are present in static data");

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_debug_mode_enabled));
    load_retptr_tag(body);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));

    push_segment_args(body, step_id);
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_breakpoint_key));
    return_if_retptr_error(body);
    load_retptr_list(body, route_ptr_local, route_len_local);

    body.instruction(&Instruction::LocalGet(route_ptr_local));
    body.instruction(&Instruction::LocalGet(route_len_local));
    push_segment_args(body, &static_data.breakpoint_hit_state);
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_checkpoint));
    load_retptr_tag(body);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    push_retptr_u8_load(body, DIRECT_CHECKPOINT_FOUND_OFFSET);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));

    push_segment_args(body, step_id);
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_breakpoint_event));
    return_if_retptr_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);

    push_segment_args(body, &static_data.breakpoint_hit_kind);
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_breakpoint_pause));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::Return);

    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_agent_debug_error(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    agent_id: u32,
    error_ptr_local: u32,
    error_len_local: u32,
    debug_ptr_local: u32,
    debug_len_local: u32,
) {
    if !track_events {
        return;
    }

    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(error_ptr_local));
    body.instruction(&Instruction::LocalGet(error_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_debug_error));
    return_if_retptr_error(body);
    load_retptr_list(body, debug_ptr_local, debug_len_local);

    push_segment_args(body, &static_data.step_debug_end_kind);
    body.instruction(&Instruction::LocalGet(debug_ptr_local));
    body.instruction(&Instruction::LocalGet(debug_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));
    return_if_retptr_error(body);
}
