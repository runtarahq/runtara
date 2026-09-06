//! Guest-owned cooperative waits for sequential component calls.
//!
//! The Agent writes its result at 0. Polling uses 128..172, after that result
//! and before the wait event at 216; neither asynchronous completion can
//! overwrite the other. Handles stay in guest locals; there is no host task
//! interface. Nonempty runtime results still use canonical ABI allocation.
use wasm_encoder::{BlockType, Function, Instruction, MemArg};

use super::abi::{emit_entry_suspend_return, emit_fail_if_retptr_error_inplace, push_retptr_arg};
use super::{DIRECT_PSPLIT_EVENT_OFFSET, DirectCoreFunctionIndices};

const TARGET: u32 = 142;
const SET: u32 = 143;
const TIMER: u32 = 144;
const STATUS: u32 = 145;
const POLL: i32 = 128;
const RETURNED: i32 = 2;
const CANCELLED: i32 = 4;
const POLL_INTERVAL_MS: i64 = 1_000;

fn mem(offset: u64) -> MemArg {
    MemArg {
        offset,
        align: 0,
        memory_index: 0,
    }
}

fn load(body: &mut Function, address: i32, offset: u64) {
    body.instruction(&Instruction::I32Const(address));
    body.instruction(&Instruction::I32Load(mem(offset)));
}

fn set_zero(body: &mut Function, local: u32) {
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(local));
}

fn load_tag(body: &mut Function, address: i32, offset: u64) {
    body.instruction(&Instruction::I32Const(address));
    body.instruction(&Instruction::I32Load8U(mem(offset)));
}

fn cancel_and_drop(body: &mut Function, indices: &DirectCoreFunctionIndices, handle: u32) {
    body.instruction(&Instruction::LocalGet(handle));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::Call(indices.waitable_join.unwrap()));
    body.instruction(&Instruction::LocalGet(handle));
    body.instruction(&Instruction::Call(indices.subtask_cancel.unwrap()));
    body.instruction(&Instruction::LocalSet(STATUS));
    // A callee may return normally while cancellation is being requested.
    body.instruction(&Instruction::LocalGet(STATUS));
    body.instruction(&Instruction::I32Const(RETURNED));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::LocalGet(STATUS));
    body.instruction(&Instruction::I32Const(CANCELLED));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::I32Or);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::Unreachable);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalGet(handle));
    body.instruction(&Instruction::Call(indices.subtask_drop.unwrap()));
    set_zero(body, handle);
}

fn close_wait(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    cancel_and_drop(body, indices, TARGET);
    body.instruction(&Instruction::LocalGet(SET));
    body.instruction(&Instruction::Call(indices.waitable_set_drop.unwrap()));
}

fn poll_error(body: &mut Function, indices: &DirectCoreFunctionIndices, pending: bool) {
    load_tag(body, POLL, 0);
    body.instruction(&Instruction::If(BlockType::Empty));
    if pending {
        close_wait(body, indices);
    }
    // Only after resolution may an error replace the Agent's result area.
    for offset in [0, 4, 8] {
        body.instruction(&Instruction::I32Const(0));
        load(body, POLL, offset);
        body.instruction(&Instruction::I32Store(mem(offset)));
    }
    emit_fail_if_retptr_error_inplace(body, indices);
    body.instruction(&Instruction::End);
}

fn poll(body: &mut Function, indices: &DirectCoreFunctionIndices, pending: bool) {
    if pending {
        body.instruction(&Instruction::I32Const(POLL));
        body.instruction(&Instruction::Call(indices.runtime_heartbeat));
        poll_error(body, indices, true);
    }
    body.instruction(&Instruction::I32Const(POLL));
    body.instruction(&Instruction::Call(indices.runtime_poll_signal));
    poll_error(body, indices, pending);
    // result<option<signal-info>, string>: option tag at 4, record at 8.
    load_tag(body, POLL, 4);
    body.instruction(&Instruction::If(BlockType::Empty));
    load(body, POLL, 12);
    body.instruction(&Instruction::I32Const(6));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    load(body, POLL, 8);
    body.instruction(&Instruction::I32Load(mem(0)));
    body.instruction(&Instruction::I32Const(i32::from_le_bytes(*b"canc")));
    body.instruction(&Instruction::I32Eq);
    load(body, POLL, 8);
    body.instruction(&Instruction::I32Load16U(mem(4)));
    body.instruction(&Instruction::I32Const(u16::from_le_bytes(*b"el") as i32));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::I32And);
    body.instruction(&Instruction::If(BlockType::Empty));
    if pending {
        close_wait(body, indices);
    }
    for offset in [8, 12, 16, 20] {
        load(body, POLL, offset);
    }
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_handle_checkpoint_signal));
    emit_fail_if_retptr_error_inplace(body, indices);
    // Rejected publication must not masquerade as completed cancellation.
    load_tag(body, 0, 4);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::Unreachable);
    body.instruction(&Instruction::End);
    emit_entry_suspend_return(body, indices);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
}

pub(super) fn emit_poll_before_call(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    if !indices.omit_runtime {
        poll(body, indices, false);
    }
}

/// Consume an async-lowered invoke's packed status, preserving its result at 0.
pub(super) fn emit_await_call(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    body.instruction(&Instruction::LocalSet(STATUS));
    body.instruction(&Instruction::LocalGet(STATUS));
    body.instruction(&Instruction::I32Const(15));
    body.instruction(&Instruction::I32And);
    body.instruction(&Instruction::I32Const(RETURNED));
    body.instruction(&Instruction::I32Ne);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(STATUS));
    body.instruction(&Instruction::I32Const(4));
    body.instruction(&Instruction::I32ShrU);
    body.instruction(&Instruction::LocalSet(TARGET));
    body.instruction(&Instruction::Call(indices.waitable_set_new.unwrap()));
    body.instruction(&Instruction::LocalSet(SET));
    body.instruction(&Instruction::LocalGet(TARGET));
    body.instruction(&Instruction::LocalGet(SET));
    body.instruction(&Instruction::Call(indices.waitable_join.unwrap()));
    set_zero(body, TIMER);
    body.instruction(&Instruction::Block(BlockType::Empty));
    body.instruction(&Instruction::Loop(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(TARGET));
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::BrIf(1));
    if !indices.omit_runtime {
        body.instruction(&Instruction::LocalGet(TIMER));
        body.instruction(&Instruction::I32Eqz);
        body.instruction(&Instruction::If(BlockType::Empty));
        poll(body, indices, true);
        body.instruction(&Instruction::I64Const(POLL_INTERVAL_MS));
        body.instruction(&Instruction::Call(indices.timer_sleep_async.unwrap()));
        body.instruction(&Instruction::LocalSet(STATUS));
        body.instruction(&Instruction::LocalGet(STATUS));
        body.instruction(&Instruction::I32Const(15));
        body.instruction(&Instruction::I32And);
        body.instruction(&Instruction::I32Const(RETURNED));
        body.instruction(&Instruction::I32Eq);
        // An eagerly completed timer is already due; poll again without waiting.
        body.instruction(&Instruction::BrIf(1));
        body.instruction(&Instruction::LocalGet(STATUS));
        body.instruction(&Instruction::I32Const(4));
        body.instruction(&Instruction::I32ShrU);
        body.instruction(&Instruction::LocalTee(TIMER));
        body.instruction(&Instruction::LocalGet(SET));
        body.instruction(&Instruction::Call(indices.waitable_join.unwrap()));
        body.instruction(&Instruction::End);
    }
    body.instruction(&Instruction::LocalGet(SET));
    body.instruction(&Instruction::I32Const(DIRECT_PSPLIT_EVENT_OFFSET));
    body.instruction(&Instruction::Call(indices.waitable_set_wait.unwrap()));
    body.instruction(&Instruction::Drop);
    load(body, DIRECT_PSPLIT_EVENT_OFFSET, 4);
    body.instruction(&Instruction::I32Const(RETURNED));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    load(body, DIRECT_PSPLIT_EVENT_OFFSET, 0);
    body.instruction(&Instruction::Call(indices.subtask_drop.unwrap()));
    load(body, DIRECT_PSPLIT_EVENT_OFFSET, 0);
    body.instruction(&Instruction::LocalGet(TARGET));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    set_zero(body, TARGET);
    body.instruction(&Instruction::Else);
    set_zero(body, TIMER);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::Br(0));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalGet(TIMER));
    body.instruction(&Instruction::If(BlockType::Empty));
    cancel_and_drop(body, indices, TIMER);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalGet(SET));
    body.instruction(&Instruction::Call(indices.waitable_set_drop.unwrap()));
    body.instruction(&Instruction::End);
}
