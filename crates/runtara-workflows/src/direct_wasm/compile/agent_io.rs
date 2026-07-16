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

use super::abi::{emit_fail_if_retptr_error_inplace, load_retptr_list, push_retptr_arg};
use super::{DirectCoreFunctionIndices, DirectCoreStaticData};

/// Inject the Agent's connection into its input under `_connection` (the single
/// connection channel — the invoke ABI has no connection argument). The stdlib
/// resolves the id against `source` (a `connection_ref` wins over the literal)
/// and rewrites the input in place; a connectionless agent is a no-op.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_agent_connection_input(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    agent_id: u32,
    input_ptr_local: u32,
    input_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
) {
    if !static_data.agent_has_connection(agent_id) {
        return;
    }

    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(input_ptr_local));
    body.instruction(&Instruction::LocalGet(input_len_local));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_connection_input));
    emit_fail_if_retptr_error_inplace(body, indices);
    load_retptr_list(body, input_ptr_local, input_len_local);
}

/// Wrap a workflow-agent child's input in the canonical `{data, variables}`
/// envelope carrying the invocation-site checkpoint namespace
/// (`variables._cache_key_prefix`). Only for workflow-agent targets — native
/// agents receive the bare input. Emitted ONCE per step, before the durable
/// checkpoint / retry loop: the wrapped buffer feeds every attempt (a second
/// pass would double-wrap), and the parent's own cache key derives from
/// `source`, not this buffer, so its dedup semantics are untouched.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_agent_scope_input(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    agent_id: u32,
    input_ptr_local: u32,
    input_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
) {
    if !static_data.agent_is_workflow_agent(agent_id) {
        return;
    }

    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(input_ptr_local));
    body.instruction(&Instruction::LocalGet(input_len_local));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_scope_input));
    emit_fail_if_retptr_error_inplace(body, indices);
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
    emit_fail_if_retptr_error_inplace(body, indices);
    load_retptr_list(body, cache_key_ptr_local, cache_key_len_local);
}
