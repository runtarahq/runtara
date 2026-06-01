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
    emit_retptr_error_or_return, load_retptr_list, push_retptr_arg, push_variables_args,
};
use super::{DirectCoreFunctionIndices, DirectFailureTarget, DirectVariables};

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
