// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Store-freeing retry/backoff parking for lifecycle-invoke workflows.
//!
//! A retry cannot keep a running component Store alive while it waits.  The
//! caller derives a distinct retry key for the *next* attempt; this helper
//! stores one absolute `u64` deadline under that key and returns
//! `suspended(at(deadline))`.  A relaunch reuses that deadline, so an early
//! manual resume re-parks and an on-time wake consumes exactly one retry.
//!
//! The retry key deliberately contains only the fixed-width deadline.  The
//! failed attempt's result/envelope is checkpointed separately by its caller,
//! which is what lets a replay reconstruct retry policy without re-invoking a
//! completed attempt.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    emit_entry_suspend_at, push_i64_load_from_ptr, push_retptr_arg, push_retptr_i64_load,
    return_if_retptr_error, store_local_i64_at,
};
use super::checkpoint::{
    emit_check_signals_and_suspend, emit_checkpoint_lookup, emit_checkpoint_save,
};
use super::{
    DIRECT_DEADLINE_SKEW_TOLERANCE_MS, DIRECT_RET_U64_OK_OFFSET,
    DIRECT_RETRY_PARK_DEADLINE_MS_LOCAL, DIRECT_RETRY_PARK_STATE_LEN_LOCAL,
    DIRECT_RETRY_PARK_STATE_PTR_LOCAL, DIRECT_WAIT_DEADLINE_SCRATCH_OFFSET,
    DirectCoreFunctionIndices,
};

/// Width of a retry park checkpoint: one absolute `u64` deadline.
///
/// Legacy inline sleeps used the same retry key with an empty state.  A state
/// whose width is not exactly eight bytes must therefore be treated as an
/// already-served legacy wait, never as a deadline that can be re-scheduled.
const RETRY_DEADLINE_STATE_LEN: i32 = 8;

/// Persist and honour an absolute retry deadline, returning from the lifecycle
/// invoke while the backoff is still owed.
///
/// `retry_key_*` identifies the next retry attempt and `delay_ms_local` holds
/// the already-clamped backoff duration.  This helper is valid only for the
/// lifecycle invoke ABI; legacy ABI callers retain their historical blocking
/// lowering because they have nowhere to return a wake.
pub(super) fn emit_retry_park_until_deadline(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    retry_key_ptr_local: u32,
    retry_key_len_local: u32,
    delay_ms_local: u32,
) {
    emit_checkpoint_lookup(
        body,
        indices,
        retry_key_ptr_local,
        retry_key_len_local,
        DIRECT_RETRY_PARK_STATE_PTR_LOCAL,
        DIRECT_RETRY_PARK_STATE_LEN_LOCAL,
    );

    // HIT: only an exact-width state is one of our absolute deadlines.  An
    // empty (or otherwise malformed) state was left by the retired blocking
    // sleep arm; falling through preserves its already-served semantics.
    body.instruction(&Instruction::LocalGet(DIRECT_RETRY_PARK_STATE_LEN_LOCAL));
    body.instruction(&Instruction::I32Const(RETRY_DEADLINE_STATE_LEN));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    push_i64_load_from_ptr(body, DIRECT_RETRY_PARK_STATE_PTR_LOCAL);
    body.instruction(&Instruction::LocalSet(DIRECT_RETRY_PARK_DEADLINE_MS_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_now_ms));
    return_if_retptr_error(body, indices);
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    // The wake scheduler compares its database clock against a deadline minted
    // from the runtime host clock.  Match Delay's small skew allowance so an
    // on-time database wake cannot spin indefinitely on a slightly early guest
    // clock.
    body.instruction(&Instruction::I64Const(DIRECT_DEADLINE_SKEW_TOLERANCE_MS));
    body.instruction(&Instruction::I64Add);
    body.instruction(&Instruction::LocalGet(DIRECT_RETRY_PARK_DEADLINE_MS_LOCAL));
    body.instruction(&Instruction::I64LtU);
    body.instruction(&Instruction::If(BlockType::Empty));
    // An operator resume is allowed to relaunch a parked instance before its
    // timed wake.  Do not shorten the retry: return the original deadline.
    emit_entry_suspend_at(body, DIRECT_RETRY_PARK_DEADLINE_MS_LOCAL);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    // The retry is due (or a legacy state was found).  A parked run did not
    // pass through a checkpoint-save on this leg, so explicitly observe a
    // cancel/pause before the next attempt can execute.
    emit_check_signals_and_suspend(body, indices);
    body.instruction(&Instruction::Else);

    // MISS: compute and checkpoint the *absolute* deadline before returning.
    // A crash after this save replays through the HIT branch above, rather than
    // minting a fresh relative delay and extending the wait.
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_now_ms));
    return_if_retptr_error(body, indices);
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::LocalGet(delay_ms_local));
    body.instruction(&Instruction::I64Add);
    body.instruction(&Instruction::LocalSet(DIRECT_RETRY_PARK_DEADLINE_MS_LOCAL));
    store_local_i64_at(
        body,
        DIRECT_WAIT_DEADLINE_SCRATCH_OFFSET,
        DIRECT_RETRY_PARK_DEADLINE_MS_LOCAL,
    );
    body.instruction(&Instruction::I32Const(DIRECT_WAIT_DEADLINE_SCRATCH_OFFSET));
    body.instruction(&Instruction::LocalSet(DIRECT_RETRY_PARK_STATE_PTR_LOCAL));
    body.instruction(&Instruction::I32Const(RETRY_DEADLINE_STATE_LEN));
    body.instruction(&Instruction::LocalSet(DIRECT_RETRY_PARK_STATE_LEN_LOCAL));
    emit_checkpoint_save(
        body,
        indices,
        retry_key_ptr_local,
        retry_key_len_local,
        DIRECT_RETRY_PARK_STATE_PTR_LOCAL,
        DIRECT_RETRY_PARK_STATE_LEN_LOCAL,
    );
    emit_entry_suspend_at(body, DIRECT_RETRY_PARK_DEADLINE_MS_LOCAL);
    body.instruction(&Instruction::End);
}
