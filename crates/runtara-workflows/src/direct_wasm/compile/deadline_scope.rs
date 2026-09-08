// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Guest-owned enclosing deadline and unwind reason. No host scope registry.
use super::*;
use wasm_encoder::{BlockType, Function, Instruction};

const OWNER: u32 = 164;
const START: u32 = 165;
const BUDGET: u32 = 166;
const ERROR_PTR: u32 = 167;
const ERROR_LEN: u32 = 168;
pub(super) const SELECTED: u32 = 169;
pub(super) const SELECTED_PTR: u32 = 170;
pub(super) const SELECTED_LEN: u32 = 171;
const CHOSEN: u32 = 172;
const ELAPSED: u32 = 173;
const FRAME: [u32; 5] = [OWNER, START, BUDGET, ERROR_PTR, ERROR_LEN];

pub(super) fn owner(id: u32, split: bool) -> i64 {
    i64::from(id) * 2 + if split { 2 } else { 1 }
}

pub(super) fn push_frame(body: &mut Function) {
    for local in FRAME {
        body.instruction(&Instruction::LocalGet(local));
    }
}
pub(super) fn pop_frame(body: &mut Function) {
    for local in FRAME.into_iter().rev() {
        body.instruction(&Instruction::LocalSet(local));
    }
}

fn remaining(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    body.instruction(&Instruction::Call(
        indices.monotonic_now.expect("deadline clock"),
    ));
    body.instruction(&Instruction::LocalGet(START));
    body.instruction(&Instruction::I64Sub);
    body.instruction(&Instruction::I64Const(1_000_000));
    body.instruction(&Instruction::I64DivU);
    body.instruction(&Instruction::LocalSet(ELAPSED));
    super::agent_deadline::subtract_saturating(body, BUDGET, ELAPSED);
}

/// The caller has converted its durable epoch deadline to remaining milliseconds.
/// Equal deadlines keep the enclosing owner, which must unwind past this scope.
pub(super) fn enter(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    id: i64,
    error: &DirectDataSegment,
    duration: u32,
) {
    body.instruction(&Instruction::LocalGet(OWNER));
    body.instruction(&Instruction::I64Eqz);
    body.instruction(&Instruction::If(BlockType::Result(
        wasm_encoder::ValType::I32,
    )));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::LocalGet(duration));
    remaining(body, indices);
    body.instruction(&Instruction::I64LtU);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::I64Const(id));
    body.instruction(&Instruction::LocalSet(OWNER));
    body.instruction(&Instruction::Call(
        indices.monotonic_now.expect("deadline clock"),
    ));
    body.instruction(&Instruction::LocalSet(START));
    body.instruction(&Instruction::LocalGet(duration));
    body.instruction(&Instruction::LocalSet(BUDGET));
    for (local, value) in [(ERROR_PTR, error.offset), (ERROR_LEN, error.len_i32())] {
        body.instruction(&Instruction::I32Const(value));
        body.instruction(&Instruction::LocalSet(local));
    }
    body.instruction(&Instruction::End);
}

/// Select the earliest live owner; the Agent's own deadline uses owner zero.
pub(super) fn choose(body: &mut Function, indices: &DirectCoreFunctionIndices, own: bool) {
    if own {
        super::agent_deadline::remaining(body, indices);
    } else {
        body.instruction(&Instruction::I64Const(-1));
        body.instruction(&Instruction::LocalSet(super::agent_deadline::REMAINING));
    }
    body.instruction(&Instruction::I64Const(0));
    body.instruction(&Instruction::LocalSet(CHOSEN));
    body.instruction(&Instruction::LocalGet(OWNER));
    body.instruction(&Instruction::I64Eqz);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    remaining(body, indices);
    body.instruction(&Instruction::LocalTee(ELAPSED));
    body.instruction(&Instruction::LocalGet(super::agent_deadline::REMAINING));
    body.instruction(&Instruction::I64LeU);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(ELAPSED));
    body.instruction(&Instruction::LocalSet(super::agent_deadline::REMAINING));
    for (dst, src) in [
        (CHOSEN, OWNER),
        (SELECTED_PTR, ERROR_PTR),
        (SELECTED_LEN, ERROR_LEN),
    ] {
        body.instruction(&Instruction::LocalGet(src));
        body.instruction(&Instruction::LocalSet(dst));
    }
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
}

pub(super) fn arm(body: &mut Function, indices: &DirectCoreFunctionIndices, own: bool) {
    choose(body, indices, own);
    if !own {
        body.instruction(&Instruction::LocalGet(CHOSEN));
        body.instruction(&Instruction::I64Eqz);
        body.instruction(&Instruction::I32Eqz);
        body.instruction(&Instruction::If(BlockType::Empty));
    }
    body.instruction(&Instruction::LocalGet(super::agent_deadline::REMAINING));
    super::cooperative_wait::arm_alarm(body, indices);
    body.instruction(&Instruction::LocalGet(super::agent_deadline::REMAINING));
    body.instruction(&Instruction::Call(
        indices.timer_sleep_async.expect("deadline timer"),
    ));
    body.instruction(&Instruction::LocalSet(
        super::cooperative_wait::DEADLINE_STATUS,
    ));
    if !own {
        body.instruction(&Instruction::End);
    }
}

pub(super) fn select(body: &mut Function) {
    body.instruction(&Instruction::LocalGet(CHOSEN));
    body.instruction(&Instruction::LocalSet(SELECTED));
}

/// Called after the owning scope frame has been restored, before its recovery.
pub(super) fn claim(body: &mut Function, id: i64) {
    body.instruction(&Instruction::LocalGet(SELECTED));
    body.instruction(&Instruction::I64Const(id));
    body.instruction(&Instruction::I64Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::I64Const(0));
    body.instruction(&Instruction::LocalSet(SELECTED));
    body.instruction(&Instruction::End);
}

pub(super) fn propagate(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    target: Option<DirectFailureTarget>,
) {
    if indices.monotonic_now.is_none() {
        return;
    }
    body.instruction(&Instruction::LocalGet(SELECTED));
    body.instruction(&Instruction::I64Eqz);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    if let Some(target) = target {
        super::split::emit_split_append_error_payload_and_continue(
            body,
            indices,
            target.nested(1),
            SELECTED_PTR,
            SELECTED_LEN,
        );
    } else {
        emit_runtime_fail_return(body, indices, SELECTED_PTR, SELECTED_LEN);
    }
    body.instruction(&Instruction::End);
}

const FAILURE_FRAME: [u32; 5] = [174, 175, 176, 177, 178];
pub(super) fn push_failure_frame(body: &mut Function) {
    for local in FAILURE_FRAME {
        body.instruction(&Instruction::LocalGet(local));
    }
}
pub(super) fn pop_failure_frame(body: &mut Function) {
    for local in FAILURE_FRAME.into_iter().rev() {
        body.instruction(&Instruction::LocalSet(local));
    }
}
pub(super) fn save_failure_frame(body: &mut Function) {
    for (dst, src) in FAILURE_FRAME.into_iter().zip(FRAME) {
        body.instruction(&Instruction::LocalGet(src));
        body.instruction(&Instruction::LocalSet(dst));
    }
}
pub(super) fn restore_failure_frame(body: &mut Function) {
    for (dst, src) in FRAME.into_iter().zip(FAILURE_FRAME) {
        body.instruction(&Instruction::LocalGet(src));
        body.instruction(&Instruction::LocalSet(dst));
    }
}

/// Leave a retry wrapper without recording an enclosing cancellation as an attempt.
pub(super) fn break_if_selected(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    depth: u32,
) {
    if indices.monotonic_now.is_none() {
        return;
    }
    body.instruction(&Instruction::LocalGet(SELECTED));
    body.instruction(&Instruction::I64Eqz);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::BrIf(depth));
}
