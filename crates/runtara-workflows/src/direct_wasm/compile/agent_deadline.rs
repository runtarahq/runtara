// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agent-owned total budget. A result checkpoint bypasses this scope; attempts
//! share its absolute deadline, including time spent in retry backoff.
use super::abi::{push_retptr_arg, push_retptr_i64_load, return_if_retptr_error};
use super::*;
use wasm_encoder::{BlockType, Function, Instruction, MemArg};

pub(super) const DEADLINE: u32 = 160;
pub(super) const REMAINING: u32 = 161;

pub(super) fn enter(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    step_id: &str,
    source: (u32, u32),
    timeout: u64,
    durable: bool,
) {
    // Public validation retains E128 while runtime-free/inherited scopes and
    // cleanup grace are qualified. Never emit a poisoned runtime clock call.
    assert!(
        !indices.omit_runtime,
        "Agent deadlines require clock lowering"
    );
    super::loop_deadline::load_budget(
        body,
        indices,
        &static_data.agent_deadline_state_error,
        static_data.step_id(step_id).expect("planned Agent"),
        source,
        timeout,
        DEADLINE,
        durable,
    );
}

/// Read remaining milliseconds without underflow. This scratch is independent
/// of loop deadline locals and does not replace an enclosing scope's budget.
pub(super) fn remaining(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_now_ms));
    return_if_retptr_error(body, indices);
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(REMAINING));
    body.instruction(&Instruction::LocalGet(DEADLINE));
    body.instruction(&Instruction::LocalGet(REMAINING));
    body.instruction(&Instruction::I64GtU);
    body.instruction(&Instruction::If(BlockType::Result(
        wasm_encoder::ValType::I64,
    )));
    body.instruction(&Instruction::LocalGet(DEADLINE));
    body.instruction(&Instruction::LocalGet(REMAINING));
    body.instruction(&Instruction::I64Sub);
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::I64Const(0));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalSet(REMAINING));
}

pub(super) fn arm(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    // Re-read after connection preparation so its time is part of the budget.
    remaining(body, indices);
    body.instruction(&Instruction::LocalGet(REMAINING));
    body.instruction(&Instruction::Call(
        indices.timer_sleep_async.expect("Agent timer"),
    ));
    body.instruction(&Instruction::LocalSet(
        super::cooperative_wait::DEADLINE_STATUS,
    ));
}

pub(super) fn clamp_retry(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    remaining(body, indices);
    body.instruction(&Instruction::LocalGet(REMAINING));
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL));
    body.instruction(&Instruction::I64LtU);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(REMAINING));
    body.instruction(&Instruction::LocalSet(DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL));
    body.instruction(&Instruction::End);
}

/// Construct the ordinary WIT error-info result after cleanup. Explicit false
/// retryability keeps cancellation from restarting a step through defaults.
pub(super) fn error(body: &mut Function, static_data: &DirectCoreStaticData) {
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Const(80));
    body.instruction(&Instruction::MemoryFill(0));
    store(body, 0, 1);
    let mut ptr = static_data.agent_timeout_error.offset;
    for (offset, text) in [8, 16, 24, 32]
        .into_iter()
        .zip(super::super::static_data::AGENT_TIMEOUT_FIELDS)
    {
        store(body, offset, ptr);
        store(body, offset + 4, text.len() as i32);
        ptr += text.len() as i32;
    }
}

fn store(body: &mut Function, offset: u64, value: i32) {
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Const(value));
    body.instruction(&Instruction::I32Store(MemArg {
        offset,
        align: 2,
        memory_index: 0,
    }));
}
