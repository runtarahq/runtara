// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agent stdlib input/cache helper lowering for the direct core emitter.
//!
//! Two small, conditional pre-invoke prep steps. `emit_agent_connection_input`
//! (only when the agent has a connection) merges connection-derived fields into
//! the input buffer; `emit_agent_cache_key` (only for durable agents) derives the
//! deterministic checkpoint key from the agent id + resolved source. Computing the
//! key from the same canonical source the step sees is what gives stable cache hits
//! across retries and replays.

use wasm_encoder::{Function as WasmFunction, Instruction};

use super::abi::{load_retptr_list, push_retptr_arg, return_if_retptr_error};
use super::{DirectCoreFunctionIndices, DirectCoreStaticData};

pub(super) fn emit_agent_connection_input(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    agent_id: u32,
    input_ptr_local: u32,
    input_len_local: u32,
) {
    if static_data.agent_connection_id(agent_id).is_none() {
        return;
    }

    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(input_ptr_local));
    body.instruction(&Instruction::LocalGet(input_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_connection_input));
    return_if_retptr_error(body);
    load_retptr_list(body, input_ptr_local, input_len_local);
}

pub(super) fn emit_agent_cache_key(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    agent_id: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    cache_key_ptr_local: u32,
    cache_key_len_local: u32,
) {
    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_cache_key));
    return_if_retptr_error(body);
    load_retptr_list(body, cache_key_ptr_local, cache_key_len_local);
}
