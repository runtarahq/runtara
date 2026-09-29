// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Core Wasm ABI and retptr helper emission.
//!
//! Every data operation is delegated to a shared component, and those imported
//! functions return their results indirectly into a fixed scratch region at
//! `DIRECT_RUN_RETPTR_OFFSET` whose first byte is an ok/err discriminant. This
//! module is the single hand-coded view of that Canonical-ABI return convention:
//! it reads the retptr (`load_retptr_*`), emits the ubiquitous "check the tag,
//! then return or branch to a `DirectFailureTarget`" error idiom, and translates
//! `wit_parser` `WasmType`s into `wasm_encoder` `ValType`s. Centralizing it keeps
//! ABI bookkeeping out of the per-step lowerers and makes error handling uniform.

use wasm_encoder::{
    BlockType, Function as WasmFunction, Ieee32, Ieee64, Instruction, MemArg, TypeSection, ValType,
};
use wit_parser::abi::WasmType;

use super::embed_workflow::emit_embed_workflow_child_error_and_continue;
use super::split::{
    emit_split_append_retptr_error_and_continue, emit_split_retry_error_and_continue,
};
use super::step_error::emit_step_error_and_continue;
use super::wait::emit_wait_on_wait_error_and_fail;
use super::{
    DIRECT_AGENT_RESULT_OK_LEN_OFFSET, DIRECT_AGENT_RESULT_OK_PTR_OFFSET,
    DIRECT_RESULT_OPTION_LIST_LEN_OFFSET, DIRECT_RESULT_OPTION_LIST_PTR_OFFSET,
    DIRECT_RESULT_OPTION_TAG_OFFSET, DIRECT_RUN_RETPTR_OFFSET, DirectCoreFunctionIndices,
    DirectCoreStaticData, DirectFailureTarget, DirectVariables,
};
use crate::direct_wasm::static_data::DirectDataSegment;

/// Store the i64 in `local` at fixed linear-memory `offset` (8-byte aligned).
pub(super) fn store_local_i64_at(function: &mut WasmFunction, offset: i32, local: u32) {
    function.instruction(&Instruction::I32Const(offset));
    function.instruction(&Instruction::LocalGet(local));
    function.instruction(&Instruction::I64Store(MemArg {
        offset: 0,
        align: 3,
        memory_index: 0,
    }));
}

/// Push the i64 at the linear-memory address held in `ptr_local` (8-byte aligned).
pub(super) fn push_i64_load_from_ptr(function: &mut WasmFunction, ptr_local: u32) {
    function.instruction(&Instruction::LocalGet(ptr_local));
    function.instruction(&Instruction::I64Load(MemArg {
        offset: 0,
        align: 3,
        memory_index: 0,
    }));
}

pub(super) fn push_segment_args(function: &mut WasmFunction, segment: &DirectDataSegment) {
    function.instruction(&Instruction::I32Const(segment.offset));
    function.instruction(&Instruction::I32Const(segment.len_i32()));
}

pub(super) fn push_variables_args(function: &mut WasmFunction, variables: DirectVariables<'_>) {
    match variables {
        DirectVariables::Segment(segment) => push_segment_args(function, segment),
        DirectVariables::Locals {
            ptr_local,
            len_local,
        } => {
            function.instruction(&Instruction::LocalGet(ptr_local));
            function.instruction(&Instruction::LocalGet(len_local));
        }
    }
}

pub(super) fn push_retptr_arg(function: &mut WasmFunction) {
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
}

/// Where every error reader (`load_retptr_list`, `return_if_retptr_error`,
/// ...) expects the `string` of a `result<T, string>`: right after the tag,
/// as for any 4-aligned `T`.
const RETPTR_ERR_PTR_OFFSET: u64 = 4;
/// Where the canonical ABI puts that `string` when `T` is 8-aligned (`u64`,
/// `option<u64>`): the payload follows the tag at +8.
pub(super) const RETPTR_WIDE_ERR_PTR_OFFSET: u64 = 8;

/// Call `index`, an import returning `result<T, string>` with an 8-aligned
/// `T` (`u64`, `option<u64>`), then move an error's string from +8/+12 to
/// +4/+8, where every shared error reader looks. The ok arm is untouched.
/// Without this, those readers took the padding at +4 as the pointer and the
/// real pointer as the length.
pub(super) fn emit_call_wide_result(function: &mut WasmFunction, index: u32) {
    function.instruction(&Instruction::Call(index));
    load_retptr_tag(function);
    function.instruction(&Instruction::If(BlockType::Empty));
    // Pointer first: the length's source (+12) is never a destination.
    for field in [0, 4] {
        function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
        function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
        function.instruction(&Instruction::I32Load(MemArg {
            offset: RETPTR_WIDE_ERR_PTR_OFFSET + field,
            align: 2,
            memory_index: 0,
        }));
        function.instruction(&Instruction::I32Store(MemArg {
            offset: RETPTR_ERR_PTR_OFFSET + field,
            align: 2,
            memory_index: 0,
        }));
    }
    function.instruction(&Instruction::End);
}

/// "Check the retptr tag; on error, exit the entry function". A bare tag
/// would be lifted as a result-area POINTER, so the retptr error bytes are
/// wrapped as `Err(error-info)`. No locals needed — the
/// message ptr/len are copied out of the retptr region before it is
/// clobbered.
pub(super) fn return_if_retptr_error(
    function: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
) {
    load_retptr_tag(function);
    function.instruction(&Instruction::If(BlockType::Empty));
    emit_invoke_err_return_from_retptr(function, indices, indices.stdlib_invoke_error_fields);
    function.instruction(&Instruction::End);
}

/// Write `Err(error-info)` into the fixed invoke result area at offset 0 and
/// `Return` its pointer, sourcing the error bytes from the retptr error
/// payload (ptr @+4, len @+8). Free of locals by design (some
/// call sites have none): the error ptr/len are staged into the low scratch
/// at @88/@92 — beyond every host call's retptr write — BEFORE anything
/// clobbers the retptr.
///
/// The structured decomposition is delegated to `stdlib.invoke-error-fields`,
/// called with the RESULT AREA as its retptr: the canonical layout of
/// `result<invoke-error, string>`'s ok arm at +8 is byte-identical to
/// error-info at +8, so all that remains is flipping the result discriminant
/// to err. If the stdlib call itself errored (infallible by construction, but
/// defended), the raw bytes ride `message` with empty structured fields.
pub(super) fn emit_invoke_err_return_from_retptr(
    function: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    stdlib_invoke_error_fields: u32,
) {
    // Stage the error ptr/len out of the retptr region.
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Load(MemArg {
        offset: 4,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 88,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Load(MemArg {
        offset: 8,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 92,
        align: 2,
        memory_index: 0,
    }));
    // Structured decomposition, written directly at the result area.
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Load(MemArg {
        offset: 88,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Load(MemArg {
        offset: 92,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I32Const(0)); // retptr = the result area
    function.instruction(&Instruction::Call(stdlib_invoke_error_fields));
    emit_invoke_err_finalize_from_scratch(function, indices);
}

/// Finalize the invoke `Err` result after `stdlib.invoke-error-fields` wrote
/// its result at the area: on the (defended) err arm fall back to the raw
/// bytes staged at @88/@92 as `message` with empty structured fields; either
/// way flip the result discriminant to err and return the area pointer.
fn emit_invoke_err_finalize_from_scratch(
    function: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
) {
    super::deadline_scope::close_alarm(function, indices);
    load_retptr_tag(function);
    function.instruction(&Instruction::If(BlockType::Empty));
    // Fallback: zero the record, message and details = staged raw bytes.
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(80));
    function.instruction(&Instruction::MemoryFill(0));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Load(MemArg {
        offset: 88,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 16,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Load(MemArg {
        offset: 92,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 20,
        align: 2,
        memory_index: 0,
    }));
    // details = some(the same raw bytes): tag @76, ptr @80, len @84.
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Store8(MemArg {
        offset: 76,
        align: 0,
        memory_index: 0,
    }));
    for (from, to) in [(88, 80), (92, 84)] {
        function.instruction(&Instruction::I32Const(0));
        function.instruction(&Instruction::I32Const(0));
        function.instruction(&Instruction::I32Load(MemArg {
            offset: from,
            align: 2,
            memory_index: 0,
        }));
        function.instruction(&Instruction::I32Store(MemArg {
            offset: to,
            align: 2,
            memory_index: 0,
        }));
    }
    function.instruction(&Instruction::End);
    // result disc = 1 (err) — on the ok arm this flips 0 -> 1 over the
    // stdlib result whose record @8 is already the error-info payload.
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Store8(MemArg {
        offset: 0,
        align: 0,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::Return);
}

/// Locals-sourced variant of the invoke `Err` writer: stage the locals into @88/@92 so the shared
/// finalizer's fallback can reach them, then decompose + finalize.
pub(super) fn emit_invoke_err_return_from_locals(
    function: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    stdlib_invoke_error_fields: u32,
    error_ptr_local: u32,
    error_len_local: u32,
) {
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalGet(error_ptr_local));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 88,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalGet(error_len_local));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 92,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::LocalGet(error_ptr_local));
    function.instruction(&Instruction::LocalGet(error_len_local));
    function.instruction(&Instruction::I32Const(0)); // retptr = the result area
    function.instruction(&Instruction::Call(stdlib_invoke_error_fields));
    emit_invoke_err_finalize_from_scratch(function, indices);
}

/// Suspend-and-exit for the entry function: the run stops early because a
/// lifecycle signal (pause/shutdown/breakpoint) was handled and the instance
/// will be re-invoked on relaunch: `Ok(outcome::suspended({[on-resume], []}))`.
///
/// The single-element wake list lives at offset 88 (past the 88-byte result
/// area, inside the reserved low-scratch region, 8-aligned; wake element stride
/// is 32). The `state` list at @20/@24 stays empty: a workflow keeps its state
/// in checkpoints. A published workflow-agent returns the same outcome to its
/// parent, which forwards it ([`emit_forward_suspended_return`]).
pub(super) fn emit_entry_suspend_return(
    function: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
) {
    super::deadline_scope::close_alarm(function, indices);
    // Zero result area + wake element (0..120).
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(120));
    function.instruction(&Instruction::MemoryFill(0));
    // result disc = 0 (ok, zeroed); outcome disc @8 = 1 (suspended).
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Store8(MemArg {
        offset: 8,
        align: 0,
        memory_index: 0,
    }));
    // list<wake> @12: ptr = 88, len = 1.
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(88));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 12,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 16,
        align: 2,
        memory_index: 0,
    }));
    // wake element @88: disc = 2 (on-resume), no payload.
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(2));
    function.instruction(&Instruction::I32Store8(MemArg {
        offset: 88,
        align: 0,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::Return);
}

/// Return from a cancelled component invocation after all nested handles have
/// resolved. The composing caller owns the cancellation decision and lifecycle
/// receipt. Returning an error is a standard RETURNED resolution, not a signal
/// acknowledgement or an error that this workflow may retry/catch.
pub(super) fn emit_entry_cancel_return(
    function: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
) {
    super::deadline_scope::close_alarm(function, indices);
    let (code, category, severity): (&[u8], &[u8], &[u8]) =
        (b"CANCELLED", b"cancellation", b"error");
    // Low scratch is safe to overwrite: every asynchronous writer has been
    // resolved, and the area is lifted as soon as this returns.
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(160));
    function.instruction(&Instruction::MemoryFill(0));
    for (address, bytes) in [(96, code), (120, category), (144, severity)] {
        for (index, chunk) in bytes.chunks(8).enumerate() {
            let mut value = [0; 8];
            value[..chunk.len()].copy_from_slice(chunk);
            function.instruction(&Instruction::I32Const(address + index as i32 * 8));
            function.instruction(&Instruction::I64Const(i64::from_le_bytes(value)));
            function.instruction(&Instruction::I64Store(MemArg {
                offset: 0,
                align: 0,
                memory_index: 0,
            }));
        }
    }
    for (field, address, bytes) in [
        (8, 96, code),
        (16, 96, code),
        (24, 120, category),
        (32, 144, severity),
    ] {
        function.instruction(&Instruction::I32Const(field));
        function.instruction(&Instruction::I32Const(address));
        function.instruction(&Instruction::I32Store(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
        function.instruction(&Instruction::I32Const(field + 4));
        function.instruction(&Instruction::I32Const(bytes.len() as i32));
        function.instruction(&Instruction::I32Store(MemArg {
            offset: 0,
            align: 2,
            memory_index: 0,
        }));
    }
    // result disc = 1 (err).
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Store8(MemArg {
        offset: 0,
        align: 0,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::Return);
}

/// Park until `deadline_local` (ms since epoch): `Ok(outcome::suspended(
/// {[at(deadline)], []}))`.
pub(super) fn emit_suspend_at_return(
    function: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    deadline_local: u32,
) {
    // This writer closes the alarm itself.
    emit_entry_suspend_at(function, indices, deadline_local);
}

/// Park on a SIGNAL, carrying the route to wait on and the wait's optional
/// timeout, so a timed wait still expires.
pub(super) fn emit_suspend_on_signal_return(
    function: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    signal_ptr_local: u32,
    signal_len_local: u32,
    deadline: Option<(u32, u32)>,
) {
    // This writer closes the alarm itself.
    emit_entry_suspend_on_signal(
        function,
        indices,
        signal_ptr_local,
        signal_len_local,
        deadline,
    );
}

/// A composed workflow-agent child answered `Ok(outcome::suspended(..))`: its
/// result, still in the retptr area, is re-raised as this workflow's own
/// suspension. On resume, replay re-invokes the child (its step checkpoint
/// never completed) and the child replays into its wait.
///
/// A single `at` or `on-signal` wake goes through the ordinary writers, which
/// clamp it to an enclosing loop's deadline. Anything else (`on-resume`,
/// `instances`, several wakes) is returned as the child built it; its `state`
/// is dropped, as a workflow's always is.
pub(super) fn emit_forward_suspended_return(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
) {
    use super::core_module::{
        NESTED_SUSPEND_DEADLINE_LOCAL, NESTED_SUSPEND_SIGNAL_LEN_LOCAL,
        NESTED_SUSPEND_SIGNAL_PTR_LOCAL,
    };
    // The wake list: ptr @12, len @16. Stash it (the ptr in the signal-ptr
    // slot) before any writer reuses the low scratch.
    push_retptr_i32_load(body, 12);
    body.instruction(&Instruction::LocalSet(NESTED_SUSPEND_SIGNAL_PTR_LOCAL));
    push_retptr_i32_load(body, 16);
    body.instruction(&Instruction::LocalSet(NESTED_SUSPEND_SIGNAL_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(NESTED_SUSPEND_SIGNAL_LEN_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    let wake = |body: &mut WasmFunction, offset: u64| {
        body.instruction(&Instruction::LocalGet(NESTED_SUSPEND_SIGNAL_PTR_LOCAL));
        body.instruction(&Instruction::I32Load8U(MemArg {
            offset,
            align: 0,
            memory_index: 0,
        }));
    };
    // A single `at(deadline)`: payload u64 at +8.
    wake(body, 0);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(NESTED_SUSPEND_SIGNAL_PTR_LOCAL));
    body.instruction(&Instruction::I64Load(MemArg {
        offset: 8,
        align: 3,
        memory_index: 0,
    }));
    body.instruction(&Instruction::LocalSet(NESTED_SUSPEND_DEADLINE_LOCAL));
    emit_entry_suspend_at(body, indices, NESTED_SUSPEND_DEADLINE_LOCAL);
    body.instruction(&Instruction::End);
    // A single `on-signal(signal-wait)`: checkpoint-id ptr/len at +8/+12,
    // deadline-ms option tag at +16, value at +24.
    wake(body, 0);
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(NESTED_SUSPEND_SIGNAL_PTR_LOCAL));
    body.instruction(&Instruction::I32Load8U(MemArg {
        offset: 16,
        align: 0,
        memory_index: 0,
    }));
    body.instruction(&Instruction::LocalSet(
        super::DIRECT_WAIT_TIMEOUT_PRESENT_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(NESTED_SUSPEND_SIGNAL_PTR_LOCAL));
    body.instruction(&Instruction::I64Load(MemArg {
        offset: 24,
        align: 3,
        memory_index: 0,
    }));
    body.instruction(&Instruction::LocalSet(super::DIRECT_WAIT_DEADLINE_MS_LOCAL));
    body.instruction(&Instruction::LocalGet(NESTED_SUSPEND_SIGNAL_PTR_LOCAL));
    body.instruction(&Instruction::I32Load(MemArg {
        offset: 12,
        align: 2,
        memory_index: 0,
    }));
    body.instruction(&Instruction::LocalSet(NESTED_SUSPEND_SIGNAL_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(NESTED_SUSPEND_SIGNAL_PTR_LOCAL));
    body.instruction(&Instruction::I32Load(MemArg {
        offset: 8,
        align: 2,
        memory_index: 0,
    }));
    body.instruction(&Instruction::LocalSet(NESTED_SUSPEND_SIGNAL_PTR_LOCAL));
    emit_entry_suspend_on_signal(
        body,
        indices,
        NESTED_SUSPEND_SIGNAL_PTR_LOCAL,
        NESTED_SUSPEND_SIGNAL_LEN_LOCAL,
        Some((
            super::DIRECT_WAIT_TIMEOUT_PRESENT_LOCAL,
            super::DIRECT_WAIT_DEADLINE_MS_LOCAL,
        )),
    );
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    // Otherwise return the child's wakes as ours, with an empty state.
    super::deadline_scope::close_alarm(body, indices);
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Const(32));
    body.instruction(&Instruction::MemoryFill(0));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Store8(MemArg {
        offset: 8,
        align: 0,
        memory_index: 0,
    }));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalGet(NESTED_SUSPEND_SIGNAL_PTR_LOCAL));
    body.instruction(&Instruction::I32Store(MemArg {
        offset: 12,
        align: 2,
        memory_index: 0,
    }));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalGet(NESTED_SUSPEND_SIGNAL_LEN_LOCAL));
    body.instruction(&Instruction::I32Store(MemArg {
        offset: 16,
        align: 2,
        memory_index: 0,
    }));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::Return);
}

/// Store-freeing suspend at a timed deadline (durable Delay under the invoke
/// export): `Ok(outcome::suspended([wake::at(deadline)]))`.
///
/// The host tears down the Store and schedules a relaunch at `deadline_local`
/// (ms since epoch) via `sleep_until`; on relaunch the replay re-reaches the
/// delay, whose sleep checkpoint now HITS and skips. The caller only invokes
/// this on the Root arm.
///
/// wake element layout (8-aligned, past the 80-byte result area): disc u8 @88
/// = 0 (at), payload u64 @96 = deadline.
pub(super) fn emit_entry_suspend_at(
    function: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    deadline_local: u32,
) {
    super::deadline_scope::close_alarm(function, indices);
    super::loop_deadline::clamp(function, deadline_local, None);
    // Zero result area + wake element (0..120).
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(120));
    function.instruction(&Instruction::MemoryFill(0));
    // result disc = 0 (ok); outcome disc @8 = 1 (suspended).
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Store8(MemArg {
        offset: 8,
        align: 0,
        memory_index: 0,
    }));
    // list<wake> @12: ptr = 88, len = 1.
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(88));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 12,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 16,
        align: 2,
        memory_index: 0,
    }));
    // wake element @88: disc = 0 (at, already zeroed); u64 deadline @96.
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalGet(deadline_local));
    function.instruction(&Instruction::I64Store(MemArg {
        offset: 96,
        align: 3,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::Return);
}

/// Store-freeing suspend on an external signal (durable Wait under the invoke
/// export): `Ok(outcome::suspended([wake::on-signal(signal-wait{checkpoint-id,
/// deadline-ms})]))`.
///
/// The host parks the instance `suspended` with `sleep_until` = the timeout
/// deadline (or NULL when `deadline_local` is `None`); the custom-signal waker
/// relaunches it when the signal arrives, and the replay re-polls the
/// (non-destructively read) signal and proceeds. The caller only invokes this
/// on the Root arm.
///
/// `signal_id_ptr_local`/`len` must reference the deterministic wait signal id
/// — a heap-allocated string well above the 0..120 result scratch, so the
/// `MemoryFill` below does not clobber it and wasmtime lifts it intact at the
/// call boundary.
///
/// `deadline` is the timeout fallback: `Some((present_flag_local, value_local))`
/// writes the `option<u64>` tag from the RUNTIME present flag (a wait's timeout
/// is dynamic) and the value from `value_local`; `None` is a wait with no
/// timeout of its own; an active enclosing loop may still supply a deadline.
///
/// wake element layout (past the 80-byte result area): disc u8 @88 = 1
/// (on-signal); signal-wait record @96 = { checkpoint-id: string (ptr @96,
/// len @100), deadline-ms: option<u64> (tag @104, value @112) }.
pub(super) fn emit_entry_suspend_on_signal(
    function: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    signal_id_ptr_local: u32,
    signal_id_len_local: u32,
    deadline: Option<(u32, u32)>,
) {
    super::deadline_scope::close_alarm(function, indices);
    // Every emitted wait carries a runtime optional deadline, including no-timeout waits.
    let deadline = match deadline {
        Some(pair) => pair,
        None => {
            function.instruction(&Instruction::I32Const(0));
            function.instruction(&Instruction::LocalSet(
                super::DIRECT_WAIT_TIMEOUT_PRESENT_LOCAL,
            ));
            (
                super::DIRECT_WAIT_TIMEOUT_PRESENT_LOCAL,
                super::DIRECT_WAIT_DEADLINE_MS_LOCAL,
            )
        }
    };
    super::loop_deadline::clamp(function, deadline.1, Some(deadline.0));

    // Zero result area + wake element (0..120).
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(120));
    function.instruction(&Instruction::MemoryFill(0));
    // result disc = 0 (ok); outcome disc @8 = 1 (suspended).
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Store8(MemArg {
        offset: 8,
        align: 0,
        memory_index: 0,
    }));
    // list<wake> @12: ptr = 88, len = 1.
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(88));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 12,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 16,
        align: 2,
        memory_index: 0,
    }));
    // wake element @88: disc = 1 (on-signal).
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::I32Store8(MemArg {
        offset: 88,
        align: 0,
        memory_index: 0,
    }));
    // signal-wait.checkpoint-id string: ptr @96, len @100.
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalGet(signal_id_ptr_local));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 96,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::LocalGet(signal_id_len_local));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 100,
        align: 2,
        memory_index: 0,
    }));
    // signal-wait.deadline-ms option<u64>: tag @104 (runtime present flag),
    // value @112. When the flag is 0 the value is ignored, so it is written
    // unconditionally; when there is no timeout at all the tag stays 0 (none).
    {
        let (present_flag_local, value_local) = deadline;
        function.instruction(&Instruction::I32Const(0));
        function.instruction(&Instruction::LocalGet(present_flag_local));
        function.instruction(&Instruction::I32Store8(MemArg {
            offset: 104,
            align: 0,
            memory_index: 0,
        }));
        function.instruction(&Instruction::I32Const(0));
        function.instruction(&Instruction::LocalGet(value_local));
        function.instruction(&Instruction::I64Store(MemArg {
            offset: 112,
            align: 3,
            memory_index: 0,
        }));
    }
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::Return);
}

/// Like `emit_fail_if_retptr_error` but reads the error list directly from the
/// retptr (no scratch locals needed) — for call sites that have no free locals.
pub(super) fn emit_fail_if_retptr_error_inplace(
    function: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
) {
    load_retptr_tag(function);
    function.instruction(&Instruction::If(BlockType::Empty));
    emit_invoke_err_return_from_retptr(function, indices, indices.stdlib_invoke_error_fields);
    function.instruction(&Instruction::End);
}

/// Return the retptr error as the run's `Err(error-info)`, which the host
/// persists. `err_ptr_local`/`err_len_local` must be free at the call site
/// (used as scratch to hold the error list).
pub(super) fn emit_fail_if_retptr_error(
    function: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    err_ptr_local: u32,
    err_len_local: u32,
) {
    load_retptr_tag(function);
    function.instruction(&Instruction::If(BlockType::Empty));
    load_retptr_list(function, err_ptr_local, err_len_local);
    super::emit_runtime_fail_return(function, indices, err_ptr_local, err_len_local);
    function.instruction(&Instruction::End);
}

pub(super) fn emit_retptr_error_or_return(
    function: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    failure_target: Option<DirectFailureTarget>,
    error_ptr_local: u32,
    error_len_local: u32,
) {
    if let Some(failure_target) = failure_target {
        emit_retptr_error_target_or_return(
            function,
            indices,
            failure_target,
            error_ptr_local,
            error_len_local,
        );
    } else {
        // No onError handler at this level: report the failure to the runtime
        // (emit a `failed` event carrying the error) before returning, rather than
        // returning `Err` from `run` with no payload. The latter makes wasmtime
        // exit non-zero with no SDK event and no diagnostic — the workflow world
        // imports no `wasi:cli/stderr`, so the error text is lost and the instance
        // is marked "crashed" with no reason.
        load_retptr_tag(function);
        function.instruction(&Instruction::If(BlockType::Empty));
        load_retptr_list(function, error_ptr_local, error_len_local);
        super::emit_runtime_fail_return(function, indices, error_ptr_local, error_len_local);
        function.instruction(&Instruction::End);
    }
}

/// Like [`emit_retptr_error_or_return`], but for an unhandled failure
/// (`failure_target` is `None`) it first emits an error-bearing `step_debug_end`
/// attributing the failure to `step_id` — whose `step_debug_start` has already
/// fired — so the per-step record carries the error and a duration rather than
/// the failure surfacing only at execution level. With an onError handler in
/// scope it routes exactly as before. `scratch_*` are free locals used to build
/// the debug payload; both branches consume the retptr error in place.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_retptr_error_or_step_fail(
    function: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    failure_target: Option<DirectFailureTarget>,
    step_id: &str,
    source_ptr_local: u32,
    source_len_local: u32,
    error_ptr_local: u32,
    error_len_local: u32,
    scratch_ptr_local: u32,
    scratch_len_local: u32,
) {
    if let Some(failure_target) = failure_target {
        emit_retptr_error_target_or_return(
            function,
            indices,
            failure_target,
            error_ptr_local,
            error_len_local,
        );
    } else {
        load_retptr_tag(function);
        function.instruction(&Instruction::If(BlockType::Empty));
        load_retptr_list(function, error_ptr_local, error_len_local);
        super::debug::emit_step_debug_error(
            function,
            indices,
            static_data,
            track_events,
            step_id,
            source_ptr_local,
            source_len_local,
            error_ptr_local,
            error_len_local,
            scratch_ptr_local,
            scratch_len_local,
        );
        super::emit_runtime_fail_return(function, indices, error_ptr_local, error_len_local);
        function.instruction(&Instruction::End);
    }
}

/// Like [`emit_retptr_error_or_step_fail`], but for a step that has NOT already
/// emitted a `step_debug_start` (Log emits no debug events on the happy path).
/// On an unhandled failure it emits both a `step_debug_start` and an error
/// `step_debug_end` so the failed step appears in the step summary, then fails;
/// successful runs still emit nothing. The failure-branch start relies on the
/// step's `debug_start_data` tolerating an unresolvable config.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_retptr_error_or_start_step_fail(
    function: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    failure_target: Option<DirectFailureTarget>,
    step_id: &str,
    source_ptr_local: u32,
    source_len_local: u32,
    error_ptr_local: u32,
    error_len_local: u32,
    scratch_ptr_local: u32,
    scratch_len_local: u32,
) {
    if let Some(failure_target) = failure_target {
        emit_retptr_error_target_or_return(
            function,
            indices,
            failure_target,
            error_ptr_local,
            error_len_local,
        );
    } else {
        load_retptr_tag(function);
        function.instruction(&Instruction::If(BlockType::Empty));
        load_retptr_list(function, error_ptr_local, error_len_local);
        super::debug::emit_step_debug_event(
            function,
            indices,
            static_data,
            track_events,
            true,
            step_id,
            source_ptr_local,
            source_len_local,
            scratch_ptr_local,
            scratch_len_local,
        );
        super::debug::emit_step_debug_error(
            function,
            indices,
            static_data,
            track_events,
            step_id,
            source_ptr_local,
            source_len_local,
            error_ptr_local,
            error_len_local,
            scratch_ptr_local,
            scratch_len_local,
        );
        super::emit_runtime_fail_return(function, indices, error_ptr_local, error_len_local);
        function.instruction(&Instruction::End);
    }
}

pub(super) fn emit_retptr_error_target_or_return(
    function: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    failure_target: DirectFailureTarget,
    error_ptr_local: u32,
    error_len_local: u32,
) {
    match failure_target {
        DirectFailureTarget::Split { .. } => emit_split_append_retptr_error_and_continue(
            function,
            indices,
            failure_target,
            error_ptr_local,
            error_len_local,
        ),
        DirectFailureTarget::SplitRetry { .. } => {
            load_retptr_tag(function);
            function.instruction(&Instruction::If(BlockType::Empty));
            load_retptr_list(function, error_ptr_local, error_len_local);
            emit_split_retry_error_and_continue(
                function,
                failure_target.nested(1),
                error_ptr_local,
                error_len_local,
            );
            function.instruction(&Instruction::End);
        }
        DirectFailureTarget::WaitOnWait { .. } => {
            load_retptr_tag(function);
            function.instruction(&Instruction::If(BlockType::Empty));
            load_retptr_list(function, error_ptr_local, error_len_local);
            emit_wait_on_wait_error_and_fail(
                function,
                indices,
                failure_target,
                error_ptr_local,
                error_len_local,
            );
            function.instruction(&Instruction::End);
        }
        DirectFailureTarget::EmbedWorkflow { .. } => {
            load_retptr_tag(function);
            function.instruction(&Instruction::If(BlockType::Empty));
            load_retptr_list(function, error_ptr_local, error_len_local);
            emit_embed_workflow_child_error_and_continue(
                function,
                failure_target.nested(1),
                error_ptr_local,
                error_len_local,
            );
            function.instruction(&Instruction::End);
        }
        DirectFailureTarget::StepError { .. } => {
            load_retptr_tag(function);
            function.instruction(&Instruction::If(BlockType::Empty));
            load_retptr_list(function, error_ptr_local, error_len_local);
            emit_step_error_and_continue(
                function,
                failure_target.nested(1),
                error_ptr_local,
                error_len_local,
            );
            function.instruction(&Instruction::End);
        }
    }
}

pub(super) fn load_retptr_tag(function: &mut WasmFunction) {
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    function.instruction(&Instruction::I32Load8U(MemArg {
        offset: 0,
        align: 0,
        memory_index: 0,
    }));
}

pub(super) fn load_retptr_list(function: &mut WasmFunction, ptr_local: u32, len_local: u32) {
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    function.instruction(&Instruction::I32Load(MemArg {
        offset: 4,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::LocalSet(ptr_local));
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    function.instruction(&Instruction::I32Load(MemArg {
        offset: 8,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::LocalSet(len_local));
}

pub(super) fn emit_get_checkpoint_has_value(function: &mut WasmFunction) {
    load_retptr_tag(function);
    function.instruction(&Instruction::I32Eqz);
    function.instruction(&Instruction::If(BlockType::Result(ValType::I32)));
    push_retptr_u8_load(function, DIRECT_RESULT_OPTION_TAG_OFFSET);
    function.instruction(&Instruction::Else);
    function.instruction(&Instruction::I32Const(0));
    function.instruction(&Instruction::End);
}

pub(super) fn load_retptr_option_list(function: &mut WasmFunction, ptr_local: u32, len_local: u32) {
    push_retptr_i32_load(function, DIRECT_RESULT_OPTION_LIST_PTR_OFFSET);
    function.instruction(&Instruction::LocalSet(ptr_local));
    push_retptr_i32_load(function, DIRECT_RESULT_OPTION_LIST_LEN_OFFSET);
    function.instruction(&Instruction::LocalSet(len_local));
}

pub(super) fn load_agent_retptr_list(function: &mut WasmFunction, ptr_local: u32, len_local: u32) {
    push_retptr_i32_load(function, DIRECT_AGENT_RESULT_OK_PTR_OFFSET);
    function.instruction(&Instruction::LocalSet(ptr_local));
    push_retptr_i32_load(function, DIRECT_AGENT_RESULT_OK_LEN_OFFSET);
    function.instruction(&Instruction::LocalSet(len_local));
}

pub(super) fn push_retptr_i32_load(function: &mut WasmFunction, offset: u64) {
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    function.instruction(&Instruction::I32Load(MemArg {
        offset,
        align: 2,
        memory_index: 0,
    }));
}

pub(super) fn push_retptr_u8_load(function: &mut WasmFunction, offset: u64) {
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    function.instruction(&Instruction::I32Load8U(MemArg {
        offset,
        align: 0,
        memory_index: 0,
    }));
}

pub(super) fn push_retptr_i64_load(function: &mut WasmFunction, offset: u64) {
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    function.instruction(&Instruction::I64Load(MemArg {
        offset,
        align: 3,
        memory_index: 0,
    }));
}

pub(super) fn zero_return_function(results: &[WasmType]) -> WasmFunction {
    let mut body = WasmFunction::new([]);
    for result in results {
        push_zero_value(&mut body, result);
    }
    body.instruction(&Instruction::End);
    body
}

pub(super) fn push_core_type(
    types: &mut TypeSection,
    type_count: &mut u32,
    params: &[WasmType],
    results: &[WasmType],
) -> u32 {
    let index = *type_count;
    *type_count += 1;
    types.ty().function(
        params.iter().map(core_val_type),
        results.iter().map(core_val_type),
    );
    index
}

fn core_val_type(ty: &WasmType) -> ValType {
    match ty {
        WasmType::I32 | WasmType::Pointer | WasmType::Length => ValType::I32,
        WasmType::I64 | WasmType::PointerOrI64 => ValType::I64,
        WasmType::F32 => ValType::F32,
        WasmType::F64 => ValType::F64,
    }
}

pub(super) fn push_zero_value(function: &mut WasmFunction, ty: &WasmType) {
    match ty {
        WasmType::I32 | WasmType::Pointer | WasmType::Length => {
            function.instruction(&Instruction::I32Const(0));
        }
        WasmType::I64 | WasmType::PointerOrI64 => {
            function.instruction(&Instruction::I64Const(0));
        }
        WasmType::F32 => {
            function.instruction(&Instruction::F32Const(Ieee32::new(0)));
        }
        WasmType::F64 => {
            function.instruction(&Instruction::F64Const(Ieee64::new(0)));
        }
    };
}
