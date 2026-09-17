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
// One effective enclosing alarm; replacing/restoring the scope uses its original
// clock and budget. No heap nodes or saved native handles are needed.
pub(super) const ALARM: u32 = 183;
const PREVIOUS_OWNER: u32 = 184;
const GRACE_BUDGET: u32 = 185;
const FRAME: [u32; 5] = [OWNER, START, BUDGET, ERROR_PTR, ERROR_LEN];

pub(super) fn owner(id: u32, split: bool) -> i64 {
    i64::from(id) * 2 + if split { 2 } else { 1 }
}

pub(super) fn push_frame(body: &mut Function) {
    for local in FRAME {
        body.instruction(&Instruction::LocalGet(local));
    }
}
pub(super) fn pop_frame(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    if indices.monotonic_now.is_some() {
        remember_alarm_owner(body);
    }
    for local in FRAME.into_iter().rev() {
        body.instruction(&Instruction::LocalSet(local));
    }
    restore_alarm(body, indices);
}

fn elapsed(body: &mut Function, clock: u32) {
    body.instruction(&Instruction::Call(clock));
    body.instruction(&Instruction::LocalGet(START));
    body.instruction(&Instruction::I64Sub);
    body.instruction(&Instruction::I64Const(1_000_000));
    body.instruction(&Instruction::I64DivU);
    body.instruction(&Instruction::LocalSet(ELAPSED));
}

fn remaining(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    elapsed(body, indices.monotonic_now.expect("deadline clock"));
    super::agent_deadline::subtract_saturating(body, BUDGET, ELAPSED);
}

pub(super) fn close_alarm(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    if indices.monotonic_now.is_none() {
        return;
    }
    body.instruction(&Instruction::LocalGet(ALARM));
    body.instruction(&Instruction::If(BlockType::Empty));
    super::cooperative_wait::cancel_and_drop(body, indices, ALARM);
    body.instruction(&Instruction::End);
}

fn arm_scope_alarm(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    // Subtract elapsed time AFTER adding grace. Restoring an overdue parent
    // must not restart a fresh five-second grace period.
    scope_alarm_remaining(body, indices.monotonic_now.expect("deadline clock"));
    super::cooperative_wait::arm_alarm_duration_into(body, indices, ALARM);
}

fn scope_alarm_remaining(body: &mut Function, clock: u32) {
    elapsed(body, clock);
    body.instruction(&Instruction::LocalGet(BUDGET));
    super::cooperative_wait::add_cleanup_grace(body);
    body.instruction(&Instruction::LocalSet(GRACE_BUDGET));
    super::agent_deadline::subtract_saturating(body, GRACE_BUDGET, ELAPSED);
}

fn remember_alarm_owner(body: &mut Function) {
    body.instruction(&Instruction::LocalGet(OWNER));
    body.instruction(&Instruction::LocalSet(PREVIOUS_OWNER));
}

fn restore_alarm(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    if indices.monotonic_now.is_none() {
        return;
    }
    body.instruction(&Instruction::LocalGet(OWNER));
    body.instruction(&Instruction::LocalGet(PREVIOUS_OWNER));
    body.instruction(&Instruction::I64Ne);
    body.instruction(&Instruction::If(BlockType::Empty));
    close_alarm(body, indices);
    body.instruction(&Instruction::LocalGet(OWNER));
    body.instruction(&Instruction::I64Eqz);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    arm_scope_alarm(body, indices);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
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
    close_alarm(body, indices);
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
    arm_scope_alarm(body, indices);
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

/// Prearm a parallel call using the same earliest owner as a sequential wait.
/// The caller moves this standard handle into its existing pending-call slot.
pub(super) fn arm_call_alarm(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    own: bool,
    handle: u32,
) {
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(handle));
    choose(body, indices, own);
    if !own {
        body.instruction(&Instruction::LocalGet(CHOSEN));
        body.instruction(&Instruction::I64Eqz);
        body.instruction(&Instruction::I32Eqz);
        body.instruction(&Instruction::If(BlockType::Empty));
    }
    body.instruction(&Instruction::LocalGet(super::agent_deadline::REMAINING));
    super::cooperative_wait::arm_alarm_into(body, indices, handle);
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
pub(super) fn restore_failure_frame(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    if indices.monotonic_now.is_some() {
        remember_alarm_owner(body);
    }
    for (dst, src) in FRAME.into_iter().zip(FAILURE_FRAME) {
        body.instruction(&Instruction::LocalGet(src));
        body.instruction(&Instruction::LocalSet(dst));
    }
    restore_alarm(body, indices);
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

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_encoder::{
        CodeSection, EntityType, ExportKind, ExportSection, FunctionSection, ImportSection, Module,
        TypeSection, ValType,
    };

    #[test]
    fn restored_scope_grace_uses_original_clock_and_saturates() {
        let mut module = Module::new();
        let mut types = TypeSection::new();
        types.ty().function([], [ValType::I64]);
        types
            .ty()
            .function([ValType::I64, ValType::I64], [ValType::I64]);
        module.section(&types);
        let mut imports = ImportSection::new();
        imports.import("clock", "now", EntityType::Function(0));
        module.section(&imports);
        let mut functions = FunctionSection::new();
        functions.function(1);
        module.section(&functions);
        let mut exports = ExportSection::new();
        exports.export("remaining", ExportKind::Func, 1);
        module.section(&exports);
        let mut body = Function::new([(184, ValType::I64)]);
        for (param, local) in [(0, START), (1, BUDGET)] {
            body.instruction(&Instruction::LocalGet(param));
            body.instruction(&Instruction::LocalSet(local));
        }
        // Execute the production arithmetic, with only the clock controlled.
        scope_alarm_remaining(&mut body, 0);
        body.instruction(&Instruction::End);
        let mut code = CodeSection::new();
        code.function(&body);
        module.section(&code);
        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(&engine, module.finish()).unwrap();
        let mut linker = wasmtime::Linker::new(&engine);
        linker
            .func_wrap("clock", "now", |caller: wasmtime::Caller<'_, u64>| {
                *caller.data()
            })
            .unwrap();
        let mut store = wasmtime::Store::new(&engine, 0);
        let instance = linker.instantiate(&mut store, &module).unwrap();
        let remaining = instance
            .get_typed_func::<(u64, u64), u64>(&mut store, "remaining")
            .unwrap();
        for (origin, elapsed_ns, budget, expected) in [
            (0, 0, 0, 5_000),
            (42, 2_000_000_000, 1_000, 4_000),
            (42, 6_000_000_000, 1_000, 0),
            (42, 7_000_000_000, 1_000, 0),
            (u64::MAX - 999_999, 999_999, 1, 5_001),
            (0, 0, u64::MAX, u64::MAX),
            (42, 1_000_000, u64::MAX, u64::MAX - 1),
            (42, 2_000_000, u64::MAX - 5_001, u64::MAX - 3),
        ] {
            *store.data_mut() = origin + elapsed_ns;
            assert_eq!(
                remaining.call(&mut store, (origin, budget)).unwrap(),
                expected
            );
        }
    }
}
