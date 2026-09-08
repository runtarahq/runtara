// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Persist loop deadlines separately from results and remember successful exits.
//! A completed loop may be replayed to rebuild its outputs after a later park;
//! its old deadline must no longer participate in enforcement or wake clamping.
use super::abi::{
    emit_get_checkpoint_has_value, load_retptr_list, load_retptr_option_list,
    push_i64_load_from_ptr, push_retptr_arg, push_retptr_i64_load, push_segment_args,
    return_if_retptr_error, store_local_i64_at,
};
use super::checkpoint::emit_checkpoint_save;
use super::*;
use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

const FRAME: [u32; 3] = [
    DIRECT_LOOP_COMPLETED_LOCAL,
    DIRECT_ACTIVE_DEADLINE_FLAG_LOCAL,
    DIRECT_ACTIVE_DEADLINE_MS_LOCAL,
];
pub(super) fn push_frame(body: &mut WasmFunction) {
    super::deadline_scope::push_frame(body);
    for local in FRAME {
        body.instruction(&Instruction::LocalGet(local));
    }
}
pub(super) fn pop_frame(body: &mut WasmFunction, indices: &DirectCoreFunctionIndices) {
    for local in FRAME.into_iter().rev() {
        body.instruction(&Instruction::LocalSet(local));
    }
    super::deadline_scope::pop_frame(body, indices);
}
fn key(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    step: &DirectDataSegment,
    source: (u32, u32),
    complete: bool,
) {
    push_segment_args(body, step);
    body.instruction(&Instruction::LocalGet(source.0));
    body.instruction(&Instruction::LocalGet(source.1));
    body.instruction(&Instruction::I32Const(i32::from(complete)));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_loop_deadline_key));
    return_if_retptr_error(body, indices);
    load_retptr_list(body, DIRECT_LOOP_KEY_PTR_LOCAL, DIRECT_LOOP_KEY_LEN_LOCAL);
}
fn lookup(body: &mut WasmFunction, indices: &DirectCoreFunctionIndices) {
    body.instruction(&Instruction::LocalGet(DIRECT_LOOP_KEY_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_LOOP_KEY_LEN_LOCAL));
    super::checkpoint::emit_get_checkpoint(body, indices);
    emit_get_checkpoint_has_value(body);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn enter(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    step_id: &str,
    source: (u32, u32),
    timeout: u64,
    deadline_local: u32,
    owner: i64,
    error: &DirectDataSegment,
) {
    let step = static_data.step_id(step_id).expect("planned loop step");
    key(body, indices, step, source, true);
    lookup(body, indices);
    body.instruction(&Instruction::LocalSet(DIRECT_LOOP_COMPLETED_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_LOOP_COMPLETED_LOCAL));
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    load_budget(
        body,
        indices,
        &static_data.loop_deadline_state_error,
        step,
        source,
        timeout,
        deadline_local,
        true,
    );
    // Convert the durable epoch deadline once for this live scope.
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_now_ms));
    return_if_retptr_error(body, indices);
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_LOOP_NOW_MS_LOCAL));
    super::agent_deadline::subtract_saturating(body, deadline_local, DIRECT_LOOP_NOW_MS_LOCAL);
    body.instruction(&Instruction::LocalSet(DIRECT_LOOP_NOW_MS_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_LOOP_NOW_MS_LOCAL));
    body.instruction(&Instruction::I64Const(timeout as i64));
    body.instruction(&Instruction::I64GtU);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::I64Const(timeout as i64));
    body.instruction(&Instruction::LocalSet(DIRECT_LOOP_NOW_MS_LOCAL));
    body.instruction(&Instruction::End);
    super::deadline_scope::enter(body, indices, owner, error, DIRECT_LOOP_NOW_MS_LOCAL);
    include_epoch(body, deadline_local);
    body.instruction(&Instruction::End);
}

/// Add a persisted scope budget to wake clamping; the caller restores its frame.
pub(super) fn include_epoch(body: &mut WasmFunction, deadline_local: u32) {
    // Add this deadline to the enclosing minimum. Frame restoration removes it.
    body.instruction(&Instruction::LocalGet(DIRECT_ACTIVE_DEADLINE_FLAG_LOCAL));
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::LocalGet(deadline_local));
    body.instruction(&Instruction::LocalGet(DIRECT_ACTIVE_DEADLINE_MS_LOCAL));
    body.instruction(&Instruction::I64LtU);
    body.instruction(&Instruction::I32Or);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(deadline_local));
    body.instruction(&Instruction::LocalSet(DIRECT_ACTIVE_DEADLINE_MS_LOCAL));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::LocalSet(DIRECT_ACTIVE_DEADLINE_FLAG_LOCAL));
}

/// Called only on successful exit, before the enclosing scope is restored.
pub(super) fn complete(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    step_id: &str,
    source: (u32, u32),
) {
    body.instruction(&Instruction::LocalGet(DIRECT_LOOP_COMPLETED_LOCAL));
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    key(
        body,
        indices,
        static_data.step_id(step_id).expect("planned loop step"),
        source,
        true,
    );
    // Empty checkpoint state is a read-only probe in the runtime. Persist a
    // nonempty marker, without checkpointing or changing the loop's outputs.
    body.instruction(&Instruction::I64Const(1));
    body.instruction(&Instruction::LocalSet(DIRECT_LOOP_NOW_MS_LOCAL));
    store_local_i64_at(
        body,
        DIRECT_WAIT_DEADLINE_SCRATCH_OFFSET,
        DIRECT_LOOP_NOW_MS_LOCAL,
    );
    body.instruction(&Instruction::I32Const(DIRECT_WAIT_DEADLINE_SCRATCH_OFFSET));
    body.instruction(&Instruction::LocalSet(DIRECT_LOOP_STATE_PTR_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::LocalSet(DIRECT_LOOP_STATE_LEN_LOCAL));
    emit_checkpoint_save(
        body,
        indices,
        DIRECT_LOOP_KEY_PTR_LOCAL,
        DIRECT_LOOP_KEY_LEN_LOCAL,
        DIRECT_LOOP_STATE_PTR_LOCAL,
        DIRECT_LOOP_STATE_LEN_LOCAL,
    );
    body.instruction(&Instruction::End);
}

/// Intersect a child's wake with the active enclosing loop budget. This only
/// changes the returned wake; the child's own durable deadline stays untouched.
pub(super) fn clamp(body: &mut WasmFunction, deadline_local: u32, present: Option<u32>) {
    body.instruction(&Instruction::LocalGet(DIRECT_ACTIVE_DEADLINE_FLAG_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_ACTIVE_DEADLINE_MS_LOCAL));
    body.instruction(&Instruction::LocalGet(deadline_local));
    body.instruction(&Instruction::I64LtU);
    if let Some(flag) = present {
        body.instruction(&Instruction::LocalGet(flag));
        body.instruction(&Instruction::I32Eqz);
        body.instruction(&Instruction::I32Or);
    }
    body.instruction(&Instruction::I32And);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(DIRECT_ACTIVE_DEADLINE_MS_LOCAL));
    body.instruction(&Instruction::LocalSet(deadline_local));
    if let Some(flag) = present {
        body.instruction(&Instruction::I32Const(1));
        body.instruction(&Instruction::LocalSet(flag));
    }
    body.instruction(&Instruction::End);
}

/// Observe the earliest live scope at a cooperative loop boundary.
pub(super) fn check(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    target: Option<DirectFailureTarget>,
) {
    if indices.monotonic_now.is_none() {
        return;
    }
    super::deadline_scope::choose(body, indices, false);
    body.instruction(&Instruction::LocalGet(super::agent_deadline::REMAINING));
    body.instruction(&Instruction::I64Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    // Root Cancel wins over scope expiry just as it does in the shared Await.
    super::cooperative_wait::emit_poll_before_call(body, indices);
    super::deadline_scope::select(body);
    super::deadline_scope::propagate(body, indices, target.map(|target| target.nested(1)));
    body.instruction(&Instruction::End);
}

/// Initialize one absolute budget. Durable callers reuse the same checkpoint
/// across attempts/replay; non-durable callers perform no checkpoint I/O.
#[allow(clippy::too_many_arguments)]
pub(super) fn load_budget(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    state_error: &DirectDataSegment,
    step: &DirectDataSegment,
    source: (u32, u32),
    timeout: u64,
    deadline_local: u32,
    durable: bool,
) {
    if durable {
        key(body, indices, step, source, false);
        lookup(body, indices);
        body.instruction(&Instruction::If(BlockType::Empty));
        load_retptr_option_list(
            body,
            DIRECT_LOOP_STATE_PTR_LOCAL,
            DIRECT_LOOP_STATE_LEN_LOCAL,
        );
        body.instruction(&Instruction::LocalGet(DIRECT_LOOP_STATE_LEN_LOCAL));
        body.instruction(&Instruction::I32Const(8));
        body.instruction(&Instruction::I32Ne);
        body.instruction(&Instruction::If(BlockType::Empty));
        body.instruction(&Instruction::I32Const(state_error.offset));
        body.instruction(&Instruction::LocalSet(DIRECT_LOOP_STATE_PTR_LOCAL));
        body.instruction(&Instruction::I32Const(state_error.len_i32()));
        body.instruction(&Instruction::LocalSet(DIRECT_LOOP_STATE_LEN_LOCAL));
        emit_runtime_fail_return(
            body,
            indices,
            DIRECT_LOOP_STATE_PTR_LOCAL,
            DIRECT_LOOP_STATE_LEN_LOCAL,
        );
        body.instruction(&Instruction::End);
        push_i64_load_from_ptr(body, DIRECT_LOOP_STATE_PTR_LOCAL);
        body.instruction(&Instruction::LocalSet(deadline_local));
        body.instruction(&Instruction::Else);
    }
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_now_ms));
    return_if_retptr_error(body, indices);
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::LocalTee(DIRECT_LOOP_NOW_MS_LOCAL));
    body.instruction(&Instruction::I64Const(timeout as i64));
    body.instruction(&Instruction::I64Add);
    body.instruction(&Instruction::LocalTee(deadline_local));
    body.instruction(&Instruction::LocalGet(DIRECT_LOOP_NOW_MS_LOCAL));
    body.instruction(&Instruction::I64LtU);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::I64Const(-1));
    body.instruction(&Instruction::LocalSet(deadline_local));
    body.instruction(&Instruction::End);
    if durable {
        store_local_i64_at(body, DIRECT_WAIT_DEADLINE_SCRATCH_OFFSET, deadline_local);
        body.instruction(&Instruction::I32Const(DIRECT_WAIT_DEADLINE_SCRATCH_OFFSET));
        body.instruction(&Instruction::LocalSet(DIRECT_LOOP_STATE_PTR_LOCAL));
        body.instruction(&Instruction::I32Const(8));
        body.instruction(&Instruction::LocalSet(DIRECT_LOOP_STATE_LEN_LOCAL));
        emit_checkpoint_save(
            body,
            indices,
            DIRECT_LOOP_KEY_PTR_LOCAL,
            DIRECT_LOOP_KEY_LEN_LOCAL,
            DIRECT_LOOP_STATE_PTR_LOCAL,
            DIRECT_LOOP_STATE_LEN_LOCAL,
        );
        body.instruction(&Instruction::End);
    }
}
