// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Runtime checkpoint lookup/save helper lowering for direct core execution.
//!
//! The durability primitive that every durable step (agent / embed / split)
//! shares. `emit_checkpoint_lookup` opens an `if` on a cache hit so the expensive
//! work runs only on a miss; `emit_checkpoint_save` persists the result and folds
//! in signal-handling — if the runtime reports a pending signal and asks to
//! suspend, the function returns early, parking the instance. Folding signal
//! handling into save is what makes a checkpoint the natural durable-suspension
//! point rather than a place that blocks.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    emit_get_checkpoint_has_value, load_retptr_option_list, load_retptr_tag, push_retptr_arg,
    push_retptr_i32_load, push_retptr_u8_load, return_if_retptr_error,
};
use super::{
    DIRECT_CHECKPOINT_PENDING_SIGNAL_TAG_OFFSET, DIRECT_CHECKPOINT_SIGNAL_TYPE_LEN_OFFSET,
    DIRECT_CHECKPOINT_SIGNAL_TYPE_PTR_OFFSET, DIRECT_RET_BOOL_OK_OFFSET, DirectCoreFunctionIndices,
};

pub(super) fn emit_checkpoint_lookup(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    cache_key_ptr_local: u32,
    cache_key_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    body.instruction(&Instruction::LocalGet(cache_key_ptr_local));
    body.instruction(&Instruction::LocalGet(cache_key_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_get_checkpoint));

    emit_get_checkpoint_has_value(body);
    body.instruction(&Instruction::If(BlockType::Empty));
    load_retptr_option_list(body, output_ptr_local, output_len_local);
}

pub(super) fn emit_checkpoint_save(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    cache_key_ptr_local: u32,
    cache_key_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    body.instruction(&Instruction::LocalGet(cache_key_ptr_local));
    body.instruction(&Instruction::LocalGet(cache_key_len_local));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_checkpoint));
    emit_checkpoint_signal_handling(body, indices);
}

fn emit_checkpoint_signal_handling(body: &mut WasmFunction, indices: &DirectCoreFunctionIndices) {
    load_retptr_tag(body);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    push_retptr_u8_load(body, DIRECT_CHECKPOINT_PENDING_SIGNAL_TAG_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    push_retptr_i32_load(body, DIRECT_CHECKPOINT_SIGNAL_TYPE_PTR_OFFSET);
    push_retptr_i32_load(body, DIRECT_CHECKPOINT_SIGNAL_TYPE_LEN_OFFSET);
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_handle_checkpoint_signal));
    return_if_retptr_error(body);
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::Return);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
}
