// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Shared Agent/Embed budget initialization and arithmetic, plus Agent error
//! construction. A result checkpoint bypasses the budget; attempts share its
//! absolute deadline, including time spent in retry backoff.
use super::abi::{push_retptr_arg, push_retptr_i64_load, return_if_retptr_error};
use super::*;
use wasm_encoder::{BlockType, Function, Instruction, MemArg};

pub(super) const DEADLINE: u32 = 160;
pub(super) const REMAINING: u32 = 161;
const START_NS: u32 = 162;
const BUDGET_MS: u32 = 163;

pub(super) fn enter(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    state_error: &DirectDataSegment,
    step: &DirectDataSegment,
    source: (u32, u32),
    timeout: u64,
    durable: bool,
) {
    if durable {
        // Epoch time is persisted so a different process can reconstruct a
        // parked budget. It is sampled only when entering this live scope.
        assert!(!indices.omit_runtime, "durable budget needs persistence");
        super::loop_deadline::load_budget(
            body,
            indices,
            state_error,
            step,
            source,
            timeout,
            DEADLINE,
            true,
        );
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.runtime_now_ms));
        return_if_retptr_error(body, indices);
        push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
        body.instruction(&Instruction::LocalSet(REMAINING));
        subtract_saturating(body, DEADLINE, REMAINING);
        body.instruction(&Instruction::LocalSet(BUDGET_MS));
        // A wall-clock rollback between runs must not grant more than the
        // authored budget. Elapsed parked time still uses the epoch deadline.
        body.instruction(&Instruction::LocalGet(BUDGET_MS));
        body.instruction(&Instruction::I64Const(timeout as i64));
        body.instruction(&Instruction::I64GtU);
        body.instruction(&Instruction::If(BlockType::Empty));
        body.instruction(&Instruction::I64Const(timeout as i64));
        body.instruction(&Instruction::LocalSet(BUDGET_MS));
        body.instruction(&Instruction::End);
    } else {
        body.instruction(&Instruction::I64Const(timeout as i64));
        body.instruction(&Instruction::LocalSet(BUDGET_MS));
    }
    body.instruction(&Instruction::Call(
        indices.monotonic_now.expect("standard clock import"),
    ));
    body.instruction(&Instruction::LocalSet(START_NS));
}

/// Remaining milliseconds from one live scope's monotonic elapsed time. Keep
/// milliseconds as a duration instead of multiplying a u64 budget by 1e6 or
/// adding it to the clock's unspecified origin: both can overflow.
pub(super) fn remaining(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    emit_remaining(body, indices.monotonic_now.expect("standard clock import"));
}

fn emit_remaining(body: &mut Function, now: u32) {
    body.instruction(&Instruction::Call(now));
    body.instruction(&Instruction::LocalGet(START_NS));
    body.instruction(&Instruction::I64Sub);
    body.instruction(&Instruction::I64Const(1_000_000));
    body.instruction(&Instruction::I64DivU);
    body.instruction(&Instruction::LocalSet(REMAINING));
    subtract_saturating(body, BUDGET_MS, REMAINING);
    body.instruction(&Instruction::LocalSet(REMAINING));
}

pub(super) fn subtract_saturating(body: &mut Function, budget: u32, elapsed: u32) {
    body.instruction(&Instruction::LocalGet(budget));
    body.instruction(&Instruction::LocalGet(elapsed));
    body.instruction(&Instruction::I64GtU);
    body.instruction(&Instruction::If(BlockType::Result(
        wasm_encoder::ValType::I64,
    )));
    body.instruction(&Instruction::LocalGet(budget));
    body.instruction(&Instruction::LocalGet(elapsed));
    body.instruction(&Instruction::I64Sub);
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::I64Const(0));
    body.instruction(&Instruction::End);
}

pub(super) fn clamp_retry(body: &mut Function, indices: &DirectCoreFunctionIndices, own: bool) {
    clamp_wait(body, indices, own, DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL);
}

/// Clamp a wait to the earliest applicable live budget.
pub(super) fn clamp_wait(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    own: bool,
    duration: u32,
) {
    super::deadline_scope::choose(body, indices, own);
    body.instruction(&Instruction::LocalGet(REMAINING));
    body.instruction(&Instruction::LocalGet(duration));
    body.instruction(&Instruction::I64LtU);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(REMAINING));
    body.instruction(&Instruction::LocalSet(duration));
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

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_encoder::{
        CodeSection, EntityType, ExportKind, ExportSection, FunctionSection, ImportSection, Module,
        TypeSection, ValType,
    };

    #[test]
    fn monotonic_budget_arithmetic_preserves_u64_durations_and_clock_origins() {
        let mut module = Module::new();
        let mut types = TypeSection::new();
        types.ty().function([], [ValType::I64]);
        types
            .ty()
            .function([ValType::I64, ValType::I64], [ValType::I64]);
        module.section(&types);
        let mut imports = ImportSection::new();
        imports.import(
            runtara_agent_wit::WASI_MONOTONIC_CLOCK_INTERFACE,
            "now",
            EntityType::Function(0),
        );
        module.section(&imports);
        let mut functions = FunctionSection::new();
        functions.function(1);
        module.section(&functions);
        let mut exports = ExportSection::new();
        exports.export("remaining", ExportKind::Func, 1);
        module.section(&exports);
        // Feed the actual emitter the canonical local slots it uses in a run.
        // Only the clock import is controlled; the arithmetic is not rewritten.
        let mut body = Function::new([(162, ValType::I64)]);
        body.instruction(&Instruction::LocalGet(0));
        body.instruction(&Instruction::LocalSet(START_NS));
        body.instruction(&Instruction::LocalGet(1));
        body.instruction(&Instruction::LocalSet(BUDGET_MS));
        emit_remaining(&mut body, 0);
        body.instruction(&Instruction::LocalGet(REMAINING));
        body.instruction(&Instruction::End);
        let mut code = CodeSection::new();
        code.function(&body);
        module.section(&code);

        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(&engine, module.finish()).unwrap();
        let mut linker = wasmtime::Linker::new(&engine);
        linker
            .func_wrap(
                runtara_agent_wit::WASI_MONOTONIC_CLOCK_INTERFACE,
                "now",
                |caller: wasmtime::Caller<'_, u64>| *caller.data(),
            )
            .unwrap();
        let mut store = wasmtime::Store::new(&engine, 0);
        let instance = linker.instantiate(&mut store, &module).unwrap();
        let remaining = instance
            .get_typed_func::<(u64, u64), u64>(&mut store, "remaining")
            .unwrap();
        for (origin, elapsed_ns, budget_ms, expected) in [
            (0, 0, 0, 0),
            (0, 0, u64::MAX, u64::MAX),
            (u64::MAX - 999_999, 999_999, 1, 1),
            (u64::MAX - 1_000_000, 1_000_000, 1, 0),
            (42, 2_000_000, 1, 0),
            (42, 1_000_000, u64::MAX, u64::MAX - 1),
            (0, u64::MAX, u64::MAX, u64::MAX - u64::MAX / 1_000_000),
        ] {
            *store.data_mut() = origin + elapsed_ns;
            assert_eq!(
                remaining.call(&mut store, (origin, budget_ms)).unwrap(),
                expected
            );
        }
    }
}
