// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Execute the production retry predicates at otherwise impractical counters.

use super::*;
use wasm_encoder::{
    CodeSection, ExportKind, ExportSection, FunctionSection, Module, TypeSection, ValType,
};

#[derive(Clone, Copy, Debug)]
enum RetryKind {
    Agent,
    Embed,
    Split,
}

fn assert_predicates(kind: RetryKind) {
    let (attempt, retryable, rate_limited, total, sleep_tag, sleep_ms) = match kind {
        RetryKind::Agent => (
            DIRECT_AGENT_RETRY_ATTEMPT_LOCAL,
            DIRECT_AGENT_RETRYABLE_LOCAL,
            DIRECT_AGENT_RATE_LIMITED_LOCAL,
            DIRECT_AGENT_RATE_LIMIT_WAIT_TOTAL_LOCAL,
            DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL,
            DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL,
        ),
        RetryKind::Embed => (
            DIRECT_EMBED_RETRY_ATTEMPT_LOCAL,
            DIRECT_EMBED_RETRYABLE_LOCAL,
            DIRECT_EMBED_RATE_LIMITED_LOCAL,
            DIRECT_EMBED_RATE_LIMIT_WAIT_TOTAL_LOCAL,
            DIRECT_EMBED_RETRY_AFTER_TAG_LOCAL,
            DIRECT_EMBED_RETRY_SLEEP_MS_LOCAL,
        ),
        RetryKind::Split => (
            DIRECT_SPLIT_RETRY_ATTEMPT_LOCAL,
            DIRECT_SPLIT_RETRYABLE_LOCAL,
            DIRECT_SPLIT_RATE_LIMITED_LOCAL,
            DIRECT_SPLIT_RATE_LIMIT_WAIT_TOTAL_LOCAL,
            DIRECT_SPLIT_RETRY_AFTER_TAG_LOCAL,
            DIRECT_SPLIT_RETRY_SLEEP_MS_LOCAL,
        ),
    };
    // (retries, current attempt, retryable, rate limited, spent, retry-after, expected)
    let cases = [
        (0, 1, true, false, 0, 0, false),
        (1, 1, true, false, 0, 0, true),
        (1, 2, true, false, 0, 0, false),
        (u32::MAX - 1, i32::MAX as u32, true, false, 0, 0, true),
        (u32::MAX - 1, i32::MAX as u32 + 1, true, false, 0, 0, true),
        (u32::MAX - 1, u32::MAX - 1, true, false, 0, 0, true),
        (u32::MAX - 1, u32::MAX, true, false, 0, 0, false),
        // Rate limits may exceed maxRetries, but may never wrap the counter.
        (0, 1, true, true, 0, 0, true),
        (0, u32::MAX - 1, true, true, 0, 0, true),
        (0, u32::MAX, true, true, 0, 0, false),
        (u32::MAX - 1, u32::MAX, true, true, 0, 0, false),
        (1, 1, true, true, 59_999, 1, true),
        (1, 1, true, true, 60_000, 1, false),
        (u32::MAX - 1, 1, false, false, 0, 0, false),
        (u32::MAX - 1, 1, false, true, 0, 0, false),
    ];
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], [ValType::I32]);
    module.section(&types);
    let mut functions = FunctionSection::new();
    let mut exports = ExportSection::new();
    let mut code = CodeSection::new();
    for (index, &(retries, current, can_retry, limited, spent, delay, _)) in
        cases.iter().enumerate()
    {
        let mut body = WasmFunction::new(core_module::CANONICAL_LOCAL_GROUPS.iter().copied());
        for (local, value) in [
            (attempt, current as i32),
            (retryable, can_retry as i32),
            (rate_limited, limited as i32),
            (sleep_tag, 1),
        ] {
            body.instruction(&Instruction::I32Const(value));
            body.instruction(&Instruction::LocalSet(local));
        }
        for (local, value) in [(total, spent), (sleep_ms, delay)] {
            body.instruction(&Instruction::I64Const(value));
            body.instruction(&Instruction::LocalSet(local));
        }
        match kind {
            RetryKind::Agent => {
                agent_retry::emit_agent_retry_condition(&mut body, retries, 0, 60_000)
            }
            RetryKind::Embed => embed_retry::emit_embed_retry_condition(&mut body, retries, 0),
            RetryKind::Split => split_retry::emit_split_retry_condition(&mut body, retries, 0),
        }
        body.instruction(&Instruction::End);
        functions.function(0);
        exports.export(&format!("case{index}"), ExportKind::Func, index as u32);
        code.function(&body);
    }
    module.section(&functions).section(&exports).section(&code);
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::new(&engine, module.finish()).unwrap();
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = wasmtime::Instance::new(&mut store, &module, &[]).unwrap();
    for (index, case) in cases.iter().enumerate() {
        let function = instance
            .get_typed_func::<(), i32>(&mut store, &format!("case{index}"))
            .unwrap();
        assert_eq!(
            function.call(&mut store, ()).unwrap(),
            i32::from(case.6),
            "{kind:?}: {case:?}"
        );
    }
}

#[test]
fn audit_06_agent_emitted_retry_predicate_respects_unsigned_ceiling() {
    assert_predicates(RetryKind::Agent);
}

#[test]
fn audit_06_embed_emitted_retry_predicate_respects_unsigned_ceiling() {
    assert_predicates(RetryKind::Embed);
}

#[test]
fn audit_06_split_emitted_retry_predicate_respects_unsigned_ceiling() {
    assert_predicates(RetryKind::Split);
}
