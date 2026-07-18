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
    emit_fail_if_retptr_error_inplace, emit_retptr_error_or_return, load_retptr_list,
    load_retptr_tag, push_retptr_arg, push_retptr_u8_load, push_segment_args,
};
use super::{
    DIRECT_CHECKPOINT_FOUND_OFFSET, DIRECT_PSPLIT_SLOT_LAUNCH_TS_OFFSET,
    DIRECT_PSPLIT_SLOT_SETTLE_TS_OFFSET, DIRECT_RET_BOOL_OK_OFFSET, DirectCoreFunctionIndices,
    DirectCoreStaticData, DirectFailureTarget,
};

/// Push the i64 at `slot_ptr_local + offset` (8-byte aligned) — a launch/settle
/// timestamp field within a parallel-window slot.
fn push_slot_i64(body: &mut WasmFunction, slot_ptr_local: u32, offset: i32) {
    body.instruction(&Instruction::LocalGet(slot_ptr_local));
    body.instruction(&Instruction::I64Load(wasm_encoder::MemArg {
        offset: offset as u64,
        align: 3,
        memory_index: 0,
    }));
}

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
    if start {
        emit_step_debug_start(
            body,
            indices,
            static_data,
            step_id,
            source_ptr_local,
            source_len_local,
            output_ptr_local,
            output_len_local,
        );
    } else {
        emit_step_debug_end(
            body,
            indices,
            static_data,
            step_id,
            source_ptr_local,
            source_len_local,
            output_ptr_local,
            output_len_local,
            None,
        );
    }
}

/// A `step_debug_end` event that carries the real `[launched, settled]` interval
/// stamped in `interval_slot_ptr_local` (a parallel-window slot). Used only by the
/// memoized Agent assemble path so the timeline/replay render true sibling overlap
/// instead of the sequential assemble cascade. Every other step emits
/// [`emit_step_debug_event`] (`start = false`), which records 0/0 (absent) and
/// falls back to assemble timing.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_step_debug_end_timed(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    step_id: &str,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    interval_slot_ptr_local: u32,
) {
    if !track_events {
        return;
    }
    emit_step_debug_end(
        body,
        indices,
        static_data,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        Some(interval_slot_ptr_local),
    );
}

#[allow(clippy::too_many_arguments)]
fn emit_step_debug_start(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    step_id: &str,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    let step_id = static_data
        .step_id(step_id)
        .expect("run plan step ids are present in static data");
    push_segment_args(body, step_id);
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_step_debug_start));
    emit_fail_if_retptr_error_inplace(body, indices);
    load_retptr_list(body, output_ptr_local, output_len_local);

    emit_custom_event(
        body,
        indices,
        &static_data.step_debug_start_kind,
        output_ptr_local,
        output_len_local,
    );
}

/// `interval_slot` supplies the `launched-at-ms` / `settled-at-ms` args — loaded
/// from the slot when present (the parallel memoized path), else 0/0 (absent).
#[allow(clippy::too_many_arguments)]
fn emit_step_debug_end(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    step_id: &str,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    interval_slot: Option<u32>,
) {
    let step_id = static_data
        .step_id(step_id)
        .expect("run plan step ids are present in static data");
    push_segment_args(body, step_id);
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    match interval_slot {
        Some(slot) => {
            push_slot_i64(body, slot, DIRECT_PSPLIT_SLOT_LAUNCH_TS_OFFSET);
            push_slot_i64(body, slot, DIRECT_PSPLIT_SLOT_SETTLE_TS_OFFSET);
        }
        None => {
            body.instruction(&Instruction::I64Const(0));
            body.instruction(&Instruction::I64Const(0));
        }
    }
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_step_debug_end));
    emit_fail_if_retptr_error_inplace(body, indices);
    load_retptr_list(body, output_ptr_local, output_len_local);

    emit_custom_event(
        body,
        indices,
        &static_data.step_debug_end_kind,
        output_ptr_local,
        output_len_local,
    );
}

/// Record a pre-built debug payload (`payload_ptr/len`) as a `custom-event` under
/// `kind` (`step_debug_start` / `step_debug_end`).
fn emit_custom_event(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    kind: &super::DirectDataSegment,
    payload_ptr_local: u32,
    payload_len_local: u32,
) {
    push_segment_args(body, kind);
    body.instruction(&Instruction::LocalGet(payload_ptr_local));
    body.instruction(&Instruction::LocalGet(payload_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));
    emit_fail_if_retptr_error_inplace(body, indices);
}

/// Emit a step-debug event for one dispatched AiAgent tool call: the stdlib
/// builds the synthetic `{ai-step}.tool.{name}.{call}` payload from the turn
/// output, then the runtime records it as a `step_debug_start`/`step_debug_end`
/// custom event — mirroring the generated loop's per-tool-call events. For the
/// end event, pass the dispatched result locals; the start event omits them.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_ai_tool_debug_event(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    agent_id: u32,
    turn_out_ptr_local: u32,
    turn_out_len_local: u32,
    tool_idx_local: u32,
    iter_local: u32,
    call_counter_local: u32,
    result_locals: Option<(u32, u32)>,
    source_ptr_local: u32,
    source_len_local: u32,
    scratch_ptr_local: u32,
    scratch_len_local: u32,
) {
    if !track_events {
        return;
    }

    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(turn_out_ptr_local));
    body.instruction(&Instruction::LocalGet(turn_out_len_local));
    body.instruction(&Instruction::LocalGet(tool_idx_local));
    body.instruction(&Instruction::LocalGet(iter_local));
    body.instruction(&Instruction::LocalGet(call_counter_local));
    if let Some((result_ptr_local, result_len_local)) = result_locals {
        body.instruction(&Instruction::LocalGet(result_ptr_local));
        body.instruction(&Instruction::LocalGet(result_len_local));
    }
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(if result_locals.is_some() {
        indices.stdlib_ai_tool_debug_end
    } else {
        indices.stdlib_ai_tool_debug_start
    }));
    emit_fail_if_retptr_error_inplace(body, indices);
    load_retptr_list(body, scratch_ptr_local, scratch_len_local);

    push_segment_args(
        body,
        if result_locals.is_some() {
            &static_data.step_debug_end_kind
        } else {
            &static_data.step_debug_start_kind
        },
    );
    body.instruction(&Instruction::LocalGet(scratch_ptr_local));
    body.instruction(&Instruction::LocalGet(scratch_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));
    emit_fail_if_retptr_error_inplace(body, indices);
}

/// Emit a step-debug event for an AiAgent conversation-memory phase
/// (load/save/compaction) as a synthetic `AiAgentMemory*` step, mirroring the
/// generated loop. `phase` uses the stdlib encoding (0 = load, 1 = save,
/// 2 = sliding-window compaction, 3 = summarize compaction). `None` locals
/// substitute the static empty-object segment. The stdlib returns an empty
/// payload for a below-threshold compaction — the event is skipped at runtime.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_ai_memory_debug_event(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    agent_id: u32,
    phase: u32,
    start: bool,
    conv_ptr_local: u32,
    conv_len_local: u32,
    state_locals: Option<(u32, u32)>,
    prior_state_locals: Option<(u32, u32)>,
    max_messages: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    scratch_ptr_local: u32,
    scratch_len_local: u32,
) {
    if !track_events {
        return;
    }

    let push_pair = |body: &mut WasmFunction, locals: Option<(u32, u32)>| match locals {
        Some((ptr_local, len_local)) => {
            body.instruction(&Instruction::LocalGet(ptr_local));
            body.instruction(&Instruction::LocalGet(len_local));
        }
        None => push_segment_args(body, &static_data.agent_empty_parameters),
    };

    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::I32Const(phase as i32));
    body.instruction(&Instruction::LocalGet(conv_ptr_local));
    body.instruction(&Instruction::LocalGet(conv_len_local));
    push_pair(body, state_locals);
    if !start {
        push_pair(body, prior_state_locals);
    }
    body.instruction(&Instruction::I32Const(max_messages as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(if start {
        indices.stdlib_ai_memory_debug_start
    } else {
        indices.stdlib_ai_memory_debug_end
    }));
    emit_fail_if_retptr_error_inplace(body, indices);
    load_retptr_list(body, scratch_ptr_local, scratch_len_local);

    // Empty payload → below-threshold compaction; skip the event.
    body.instruction(&Instruction::LocalGet(scratch_len_local));
    body.instruction(&Instruction::If(BlockType::Empty));
    push_segment_args(
        body,
        if start {
            &static_data.step_debug_start_kind
        } else {
            &static_data.step_debug_end_kind
        },
    );
    body.instruction(&Instruction::LocalGet(scratch_ptr_local));
    body.instruction(&Instruction::LocalGet(scratch_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));
    emit_fail_if_retptr_error_inplace(body, indices);
    body.instruction(&Instruction::End);
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
    emit_fail_if_retptr_error_inplace(body, indices);
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
    emit_fail_if_retptr_error_inplace(body, indices);
    load_retptr_list(body, output_ptr_local, output_len_local);

    push_segment_args(body, &static_data.breakpoint_hit_kind);
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_breakpoint_pause));
    // Suspend-and-exit: ABI-aware (clean-run tag vs suspended outcome).
    super::abi::emit_entry_suspend_return(body, indices);

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
    source_ptr_local: u32,
    source_len_local: u32,
    error_ptr_local: u32,
    error_len_local: u32,
    debug_ptr_local: u32,
    debug_len_local: u32,
) {
    if !track_events {
        return;
    }

    // Pass the step source so the failed-agent debug-end carries the same
    // scope_id / loop_indices as its debug-start — otherwise a failed agent
    // inside a Split/While iteration can't be paired with its start.
    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    body.instruction(&Instruction::LocalGet(error_ptr_local));
    body.instruction(&Instruction::LocalGet(error_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_debug_error));
    emit_fail_if_retptr_error_inplace(body, indices);
    load_retptr_list(body, debug_ptr_local, debug_len_local);

    push_segment_args(body, &static_data.step_debug_end_kind);
    body.instruction(&Instruction::LocalGet(debug_ptr_local));
    body.instruction(&Instruction::LocalGet(debug_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));
    emit_fail_if_retptr_error_inplace(body, indices);
}

/// Emit an error-bearing `step_debug_end` for a failed step of any type — the
/// generic analogue of [`emit_agent_debug_error`], keyed by step id. Used to
/// attribute an unhandled input-resolution failure (mapping/condition/config) to
/// the step whose `step_debug_start` already fired, so the step summary pairs
/// them into a failed record carrying the error and a duration. `error_*` hold
/// the captured error list; `debug_*` are free scratch for the built payload.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_step_debug_error(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    step_id: &str,
    source_ptr_local: u32,
    source_len_local: u32,
    error_ptr_local: u32,
    error_len_local: u32,
    debug_ptr_local: u32,
    debug_len_local: u32,
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
    body.instruction(&Instruction::LocalGet(error_ptr_local));
    body.instruction(&Instruction::LocalGet(error_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_step_debug_error));
    emit_fail_if_retptr_error_inplace(body, indices);
    load_retptr_list(body, debug_ptr_local, debug_len_local);

    push_segment_args(body, &static_data.step_debug_end_kind);
    body.instruction(&Instruction::LocalGet(debug_ptr_local));
    body.instruction(&Instruction::LocalGet(debug_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_custom_event));
    emit_fail_if_retptr_error_inplace(body, indices);
}
