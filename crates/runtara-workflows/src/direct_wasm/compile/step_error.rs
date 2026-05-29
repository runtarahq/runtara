// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Generic step-level error capture used by direct `onError` wrappers.

use wasm_encoder::{Function as WasmFunction, Instruction};

use super::{
    DIRECT_STEP_ERROR_FLAG_LOCAL, DIRECT_STEP_ERROR_LEN_LOCAL, DIRECT_STEP_ERROR_PTR_LOCAL,
    DirectFailureTarget,
};

pub(super) fn push_step_error_frame(body: &mut WasmFunction) {
    body.instruction(&Instruction::LocalGet(DIRECT_STEP_ERROR_FLAG_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_STEP_ERROR_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_STEP_ERROR_LEN_LOCAL));
}

pub(super) fn pop_step_error_frame(body: &mut WasmFunction) {
    body.instruction(&Instruction::LocalSet(DIRECT_STEP_ERROR_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_STEP_ERROR_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_STEP_ERROR_FLAG_LOCAL));
}

pub(super) fn emit_step_error_and_continue(
    body: &mut WasmFunction,
    target: DirectFailureTarget,
    error_ptr_local: u32,
    error_len_local: u32,
) {
    let DirectFailureTarget::StepError { branch_depth } = target else {
        panic!("StepError failure target expected");
    };

    body.instruction(&Instruction::LocalGet(error_ptr_local));
    body.instruction(&Instruction::LocalSet(DIRECT_STEP_ERROR_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(error_len_local));
    body.instruction(&Instruction::LocalSet(DIRECT_STEP_ERROR_LEN_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::LocalSet(DIRECT_STEP_ERROR_FLAG_LOCAL));
    body.instruction(&Instruction::Br(branch_depth));
}
