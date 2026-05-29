// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Split retry and backoff helper lowering.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction, ValType};

use super::abi::{
    load_retptr_list, push_retptr_arg, push_retptr_i64_load, push_retptr_u8_load,
    push_segment_args, return_if_retptr_error,
};
use super::{
    DIRECT_RESULT_OPTION_U64_TAG_OFFSET, DIRECT_RESULT_OPTION_U64_VALUE_OFFSET,
    DIRECT_RET_BOOL_OK_OFFSET, DIRECT_RET_U64_OK_OFFSET, DIRECT_SPLIT_RATE_LIMIT_WAIT_TOTAL_LOCAL,
    DIRECT_SPLIT_RATE_LIMITED_LOCAL, DIRECT_SPLIT_RETRY_AFTER_TAG_LOCAL,
    DIRECT_SPLIT_RETRY_ATTEMPT_LOCAL, DIRECT_SPLIT_RETRY_ERROR_LEN_LOCAL,
    DIRECT_SPLIT_RETRY_ERROR_PTR_LOCAL, DIRECT_SPLIT_RETRY_SLEEP_KEY_LEN_LOCAL,
    DIRECT_SPLIT_RETRY_SLEEP_KEY_PTR_LOCAL, DIRECT_SPLIT_RETRY_SLEEP_MS_LOCAL,
    DIRECT_SPLIT_RETRYABLE_LOCAL, DirectCoreFunctionIndices, DirectCoreStaticData,
};

const SPLIT_RETRY_MAX_DELAY_MS: u64 = 60_000;

pub(super) fn emit_split_retry_error_info(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
) {
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ERROR_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ERROR_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_workflow_error_retryable));
    return_if_retptr_error(body);
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RETRYABLE_LOCAL));

    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ERROR_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ERROR_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(
        indices.stdlib_workflow_error_rate_limited,
    ));
    return_if_retptr_error(body);
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RATE_LIMITED_LOCAL));

    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ERROR_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ERROR_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(
        indices.stdlib_workflow_error_retry_after_ms,
    ));
    return_if_retptr_error(body);
    push_retptr_u8_load(body, DIRECT_RESULT_OPTION_U64_TAG_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RETRY_AFTER_TAG_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_AFTER_TAG_LOCAL));
    body.instruction(&Instruction::If(BlockType::Empty));
    push_retptr_i64_load(body, DIRECT_RESULT_OPTION_U64_VALUE_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RETRY_SLEEP_MS_LOCAL));
    body.instruction(&Instruction::End);
}

pub(super) fn emit_split_retry_condition(
    body: &mut WasmFunction,
    max_retries: u32,
    retry_delay_ms: u64,
) {
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRYABLE_LOCAL));
    body.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RATE_LIMITED_LOCAL));
    body.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_RATE_LIMIT_WAIT_TOTAL_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_AFTER_TAG_LOCAL));
    body.instruction(&Instruction::If(BlockType::Result(ValType::I64)));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_SLEEP_MS_LOCAL));
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::I64Const(retry_delay_ms as i64));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::I64Add);
    body.instruction(&Instruction::LocalSet(
        DIRECT_SPLIT_RATE_LIMIT_WAIT_TOTAL_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_RATE_LIMIT_WAIT_TOTAL_LOCAL,
    ));
    body.instruction(&Instruction::I64Const(SPLIT_RETRY_MAX_DELAY_MS as i64));
    body.instruction(&Instruction::I64LeU);
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::I32Const(max_retries as i32));
    body.instruction(&Instruction::I32LeU);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::End);
}

pub(super) fn emit_split_advance_retry_attempt(body: &mut WasmFunction) {
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RETRY_ATTEMPT_LOCAL));
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_split_retry_before_attempt(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    durable: bool,
    cache_key_ptr_local: u32,
    cache_key_len_local: u32,
    max_retries: u32,
    retry_delay_ms: u64,
) {
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32GtU);
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_split_retry_delay(body, indices, max_retries, retry_delay_ms);
    emit_split_retry_sleep(
        body,
        indices,
        static_data,
        durable,
        cache_key_ptr_local,
        cache_key_len_local,
    );
    if durable {
        emit_split_record_retry_attempt(body, indices, cache_key_ptr_local, cache_key_len_local);
    }
    body.instruction(&Instruction::End);
}

fn emit_split_retry_delay(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    max_retries: u32,
    retry_delay_ms: u64,
) {
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::I32Const((max_retries + 1) as i32));
    body.instruction(&Instruction::I64Const(retry_delay_ms as i64));
    body.instruction(&Instruction::I64Const(SPLIT_RETRY_MAX_DELAY_MS as i64));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_AFTER_TAG_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_SLEEP_MS_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_retry_delay_ms));
    return_if_retptr_error(body);
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RETRY_SLEEP_MS_LOCAL));
}

fn emit_split_retry_sleep(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    durable: bool,
    cache_key_ptr_local: u32,
    cache_key_len_local: u32,
) {
    if durable {
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_AFTER_TAG_LOCAL));
        body.instruction(&Instruction::If(BlockType::Empty));
        body.instruction(&Instruction::LocalGet(cache_key_ptr_local));
        body.instruction(&Instruction::LocalGet(cache_key_len_local));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ATTEMPT_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_retry_sleep_key));
        return_if_retptr_error(body);
        load_retptr_list(
            body,
            DIRECT_SPLIT_RETRY_SLEEP_KEY_PTR_LOCAL,
            DIRECT_SPLIT_RETRY_SLEEP_KEY_LEN_LOCAL,
        );

        body.instruction(&Instruction::LocalGet(
            DIRECT_SPLIT_RETRY_SLEEP_KEY_PTR_LOCAL,
        ));
        body.instruction(&Instruction::LocalGet(
            DIRECT_SPLIT_RETRY_SLEEP_KEY_LEN_LOCAL,
        ));
        push_segment_args(body, &static_data.agent_rate_limit_wait);
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_SLEEP_MS_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.runtime_durable_sleep_checkpoint));
        return_if_retptr_error(body);
        body.instruction(&Instruction::Else);
        emit_blocking_sleep(body, indices);
        body.instruction(&Instruction::End);
    } else {
        emit_blocking_sleep(body, indices);
    }
}

fn emit_blocking_sleep(body: &mut WasmFunction, indices: &DirectCoreFunctionIndices) {
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_SLEEP_MS_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_blocking_sleep));
    return_if_retptr_error(body);
}

fn emit_split_record_retry_attempt(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    cache_key_ptr_local: u32,
    cache_key_len_local: u32,
) {
    body.instruction(&Instruction::LocalGet(cache_key_ptr_local));
    body.instruction(&Instruction::LocalGet(cache_key_len_local));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ERROR_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ERROR_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_record_retry_attempt));
    return_if_retptr_error(body);
}
