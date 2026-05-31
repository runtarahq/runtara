// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agent retry and backoff helper lowering for the direct core emitter.
//!
//! The retry/backoff state machine that `agent.rs` stitches into its invoke loop,
//! split into individually-testable emit helpers. Retry policy is subtle — a
//! separate budget for rate-limit waits vs. an attempt count for normal retryable
//! errors, and a server-supplied `retry-after` taking precedence over computed
//! exponential backoff — so each decision (classify error, compute the predicate,
//! compute the delay, sleep, record the attempt) is its own helper. Sleeps use
//! durable per-attempt checkpoint keys so a resumed instance doesn't re-sleep an
//! already-elapsed delay.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction, ValType};

use super::abi::{
    load_retptr_list, push_retptr_arg, push_retptr_i32_load, push_retptr_i64_load,
    push_retptr_u8_load, push_segment_args, return_if_retptr_error,
};
use super::{
    DIRECT_AGENT_RATE_LIMIT_WAIT_TOTAL_LOCAL, DIRECT_AGENT_RATE_LIMITED_LOCAL,
    DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_LEN_OFFSET, DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_PTR_OFFSET,
    DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_TAG_OFFSET, DIRECT_AGENT_RESULT_ERR_CATEGORY_LEN_OFFSET,
    DIRECT_AGENT_RESULT_ERR_CATEGORY_PTR_OFFSET, DIRECT_AGENT_RESULT_ERR_CODE_LEN_OFFSET,
    DIRECT_AGENT_RESULT_ERR_CODE_PTR_OFFSET, DIRECT_AGENT_RESULT_ERR_MESSAGE_LEN_OFFSET,
    DIRECT_AGENT_RESULT_ERR_MESSAGE_PTR_OFFSET, DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_TAG_OFFSET,
    DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_VALUE_OFFSET, DIRECT_AGENT_RESULT_ERR_RETRYABLE_OFFSET,
    DIRECT_AGENT_RESULT_ERR_SEVERITY_LEN_OFFSET, DIRECT_AGENT_RESULT_ERR_SEVERITY_PTR_OFFSET,
    DIRECT_AGENT_RETRY_ATTEMPT_LOCAL, DIRECT_AGENT_RETRY_INFO_PAYLOAD_LEN_OFFSET,
    DIRECT_AGENT_RETRY_INFO_PAYLOAD_PTR_OFFSET, DIRECT_AGENT_RETRY_INFO_RATE_LIMITED_OFFSET,
    DIRECT_AGENT_RETRY_INFO_RETRYABLE_OFFSET, DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL,
    DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL, DIRECT_AGENT_RETRYABLE_LOCAL, DIRECT_RET_U64_OK_OFFSET,
    DirectCoreFunctionIndices, DirectCoreStaticData,
};

pub(super) fn emit_agent_retry_condition(
    body: &mut WasmFunction,
    max_retries: u32,
    retry_delay_ms: u64,
    rate_limit_budget_ms: u64,
) {
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRYABLE_LOCAL));
    body.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RATE_LIMITED_LOCAL));
    body.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
    body.instruction(&Instruction::LocalGet(
        DIRECT_AGENT_RATE_LIMIT_WAIT_TOTAL_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL));
    body.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL));
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::I64Const(retry_delay_ms as i64));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::I64Add);
    body.instruction(&Instruction::LocalSet(
        DIRECT_AGENT_RATE_LIMIT_WAIT_TOTAL_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(
        DIRECT_AGENT_RATE_LIMIT_WAIT_TOTAL_LOCAL,
    ));
    body.instruction(&Instruction::I64Const(rate_limit_budget_ms as i64));
    body.instruction(&Instruction::I64LeU);
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::I32Const(max_retries as i32));
    body.instruction(&Instruction::I32LeU);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::End);
}

pub(super) fn emit_agent_advance_retry_attempt(body: &mut WasmFunction) {
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_AGENT_RETRY_ATTEMPT_LOCAL));
}

pub(super) fn emit_agent_capture_retry_sleep(body: &mut WasmFunction) {
    push_retptr_u8_load(body, DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_TAG_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL));
    body.instruction(&Instruction::If(BlockType::Empty));
    push_retptr_i64_load(body, DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_VALUE_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL));
    body.instruction(&Instruction::End);
}

pub(super) fn emit_agent_retry_delay(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    max_retries: u32,
    retry_delay_ms: u64,
    rate_limit_budget_ms: u64,
) {
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::I32Const((max_retries + 1) as i32));
    body.instruction(&Instruction::I64Const(retry_delay_ms as i64));
    body.instruction(&Instruction::I64Const(rate_limit_budget_ms as i64));
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_retry_delay_ms));
    return_if_retptr_error(body);
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL));
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_agent_retry_sleep(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    durable_checkpoint: bool,
    cache_key_ptr_local: u32,
    cache_key_len_local: u32,
    sleep_key_ptr_local: u32,
    sleep_key_len_local: u32,
) {
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL));
    body.instruction(&Instruction::If(BlockType::Empty));
    if durable_checkpoint {
        body.instruction(&Instruction::LocalGet(cache_key_ptr_local));
        body.instruction(&Instruction::LocalGet(cache_key_len_local));
        body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_ATTEMPT_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_agent_retry_sleep_key));
        return_if_retptr_error(body);
        load_retptr_list(body, sleep_key_ptr_local, sleep_key_len_local);

        body.instruction(&Instruction::LocalGet(sleep_key_ptr_local));
        body.instruction(&Instruction::LocalGet(sleep_key_len_local));
        push_segment_args(body, &static_data.agent_rate_limit_wait);
    }
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL));
    if durable_checkpoint {
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.runtime_durable_sleep_checkpoint));
        return_if_retptr_error(body);
    } else {
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.runtime_blocking_sleep));
        return_if_retptr_error(body);
    }
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(if durable_checkpoint {
        indices.runtime_durable_sleep
    } else {
        indices.runtime_blocking_sleep
    }));
    return_if_retptr_error(body);
    body.instruction(&Instruction::End);
}

pub(super) fn emit_agent_record_retry_attempt(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    cache_key_ptr_local: u32,
    cache_key_len_local: u32,
    error_ptr_local: u32,
    error_len_local: u32,
) {
    body.instruction(&Instruction::LocalGet(cache_key_ptr_local));
    body.instruction(&Instruction::LocalGet(cache_key_len_local));
    body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::LocalGet(error_ptr_local));
    body.instruction(&Instruction::LocalGet(error_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_record_retry_attempt));
    return_if_retptr_error(body);
}

pub(super) fn emit_agent_retry_error_info(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_CODE_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_CODE_LEN_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_MESSAGE_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_MESSAGE_LEN_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_CATEGORY_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_CATEGORY_LEN_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_SEVERITY_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_SEVERITY_LEN_OFFSET);
    push_retptr_u8_load(body, DIRECT_AGENT_RESULT_ERR_RETRYABLE_OFFSET);
    push_retptr_u8_load(body, DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_TAG_OFFSET);
    push_retptr_i64_load(body, DIRECT_AGENT_RESULT_ERR_RETRY_AFTER_VALUE_OFFSET);
    push_retptr_u8_load(body, DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_TAG_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_AGENT_RESULT_ERR_ATTRIBUTES_LEN_OFFSET);
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_retry_error_info));
    return_if_retptr_error(body);
    push_retptr_i32_load(body, DIRECT_AGENT_RETRY_INFO_PAYLOAD_PTR_OFFSET);
    body.instruction(&Instruction::LocalSet(output_ptr_local));
    push_retptr_i32_load(body, DIRECT_AGENT_RETRY_INFO_PAYLOAD_LEN_OFFSET);
    body.instruction(&Instruction::LocalSet(output_len_local));
    push_retptr_u8_load(body, DIRECT_AGENT_RETRY_INFO_RETRYABLE_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_AGENT_RETRYABLE_LOCAL));
    push_retptr_u8_load(body, DIRECT_AGENT_RETRY_INFO_RATE_LIMITED_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_AGENT_RATE_LIMITED_LOCAL));
}
