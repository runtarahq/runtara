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

pub(super) fn store_i32_at(function: &mut WasmFunction, offset: i32, value: i32) {
    function.instruction(&Instruction::I32Const(offset));
    function.instruction(&Instruction::I32Const(value));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }));
}

pub(super) fn store_local_i32_at(function: &mut WasmFunction, offset: i32, local: u32) {
    function.instruction(&Instruction::I32Const(offset));
    function.instruction(&Instruction::LocalGet(local));
    function.instruction(&Instruction::I32Store(MemArg {
        offset: 0,
        align: 2,
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

pub(super) fn return_if_retptr_error(function: &mut WasmFunction) {
    load_retptr_tag(function);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::Return);
    function.instruction(&Instruction::End);
}

/// Like `emit_fail_if_retptr_error` but reads the error list directly from the
/// retptr (no scratch locals needed) — for call sites that have no free locals.
pub(super) fn emit_fail_if_retptr_error_inplace(
    function: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
) {
    load_retptr_tag(function);
    function.instruction(&Instruction::If(BlockType::Empty));
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    function.instruction(&Instruction::I32Load(MemArg {
        offset: 4,
        align: 2,
        memory_index: 0,
    }));
    function.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    function.instruction(&Instruction::I32Load(MemArg {
        offset: 8,
        align: 2,
        memory_index: 0,
    }));
    push_retptr_arg(function);
    function.instruction(&Instruction::Call(indices.runtime_fail));
    function.instruction(&Instruction::I32Const(1));
    function.instruction(&Instruction::Return);
    function.instruction(&Instruction::End);
}

/// Like `return_if_retptr_error`, but reports the error via `runtime.fail`
/// (emitting a `failed` SDK event carrying the error) before returning, instead
/// of returning `Err` with no payload — which makes wasmtime exit non-zero with
/// no SDK event and no diagnostic. `err_ptr_local`/`err_len_local` must be free
/// at the call site (used as scratch to hold the error list).
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
