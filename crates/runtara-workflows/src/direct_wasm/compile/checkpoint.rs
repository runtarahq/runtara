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
    emit_get_checkpoint_has_value, load_retptr_option_list, push_retptr_arg, push_retptr_u8_load,
    return_if_retptr_error,
};
use super::{DIRECT_RET_BOOL_OK_OFFSET, DirectCoreFunctionIndices};

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

/// Poll for a pending lifecycle signal and suspend the run if one is acted on.
///
/// The standalone counterpart to the signal handling folded into
/// `emit_checkpoint_save`: a step that blocks without writing a checkpoint has
/// no save to fold into, so it needs an explicit poll site. Without one a
/// cancel that arrives while the step is blocked is never observed, and a run
/// made only of such steps contains no poll site at all.
pub(super) fn emit_check_signals_and_suspend(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
) {
    super::cooperative_wait::emit_if_safe_boundary(body, indices);
    if indices.omit_runtime {
        // The shared boundary already yielded to parent cancellation. Callable
        // workflows cannot consume the root signal; hostless entries have no
        // yield import and simply continue here.
        body.instruction(&Instruction::End);
        return;
    }
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_check_signals));
    return_if_retptr_error(body, indices);
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    // Suspend-and-exit: ABI-aware (clean-run tag vs suspended outcome).
    super::abi::emit_entry_suspend_return(body, indices);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End); // if safe boundary
}

fn emit_checkpoint_signal_handling(body: &mut WasmFunction, indices: &DirectCoreFunctionIndices) {
    super::cooperative_wait::emit_checkpoint_signal(body, indices);
}
