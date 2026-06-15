// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Source-envelope and mapping helper lowering for the direct core emitter.
//!
//! The two most-repeated stdlib calls in any lowering. `emit_build_source`
//! assembles a step's evaluation context (`data` + workflow `variables` +
//! accumulated step outputs) via `stdlib_build_source`; `emit_apply_mapping` runs
//! a DSL mapping over that source via `stdlib_apply_mapping`, addressing the
//! mapping by its numeric manifest id. Every step rebuilds its source after
//! producing output (so downstream `steps.X` references resolve) and most apply a
//! mapping, so factoring these out keeps the lowerers small and guarantees every
//! step constructs its context identically.

use wasm_encoder::{Function as WasmFunction, Instruction};

use super::abi::{
    emit_retptr_error_or_return, emit_retptr_error_or_start_step_fail,
    emit_retptr_error_or_step_fail, load_retptr_list, push_retptr_arg, push_variables_args,
};
use super::{
    DirectCoreFunctionIndices, DirectCoreStaticData, DirectFailureTarget, DirectVariables,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_build_source(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    variables: DirectVariables<'_>,
    data_ptr_local: u32,
    data_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    failure_target: Option<DirectFailureTarget>,
) {
    body.instruction(&Instruction::LocalGet(data_ptr_local));
    body.instruction(&Instruction::LocalGet(data_len_local));
    push_variables_args(body, variables);
    body.instruction(&Instruction::LocalGet(steps_ptr_local));
    body.instruction(&Instruction::LocalGet(steps_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_build_source));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        source_ptr_local,
        source_len_local,
    );
    load_retptr_list(body, source_ptr_local, source_len_local);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_apply_mapping(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    mapping_id: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    failure_target: Option<DirectFailureTarget>,
) {
    body.instruction(&Instruction::I32Const(mapping_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_apply_mapping));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        output_ptr_local,
        output_len_local,
    );
    load_retptr_list(body, output_ptr_local, output_len_local);
}

/// Like [`emit_apply_mapping`], but on an unhandled failure attributes the error
/// to `step_id` (whose `step_debug_start` has already fired) via an error
/// `step_debug_end` before failing — for steps whose input mapping resolves
/// after their start event (Finish). `scratch_*` are free locals for the debug
/// payload.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_apply_mapping_step_error(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    mapping_id: u32,
    step_id: &str,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    scratch_ptr_local: u32,
    scratch_len_local: u32,
    failure_target: Option<DirectFailureTarget>,
) {
    body.instruction(&Instruction::I32Const(mapping_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_apply_mapping));
    emit_retptr_error_or_step_fail(
        body,
        indices,
        static_data,
        track_events,
        failure_target,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        scratch_ptr_local,
        scratch_len_local,
    );
    load_retptr_list(body, output_ptr_local, output_len_local);
}

/// Apply an input mapping for a step whose `step_debug_start` fires *after* its
/// mapping (Agent, EmbedWorkflow), attributing an unhandled failure to the step.
///
/// Because the start event has not fired yet at the failure point, on an
/// unhandled mapping error this emits both a `step_debug_start` and an error
/// `step_debug_end` for `step_id` (so the step summary pairs them into a failed
/// record), then fails with the same error. With an onError handler it routes as
/// the plain [`emit_apply_mapping`] does. The failure-branch start relies on the
/// step's `debug_start_data` tolerating an unresolvable mapping. `scratch_*` are
/// free locals for the debug payloads. No-op debug events when events are off.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_apply_mapping_start_step_error(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    mapping_id: u32,
    step_id: &str,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    scratch_ptr_local: u32,
    scratch_len_local: u32,
    failure_target: Option<DirectFailureTarget>,
) {
    body.instruction(&Instruction::I32Const(mapping_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_apply_mapping));
    emit_retptr_error_or_start_step_fail(
        body,
        indices,
        static_data,
        track_events,
        failure_target,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        scratch_ptr_local,
        scratch_len_local,
    );
    load_retptr_list(body, output_ptr_local, output_len_local);
}
