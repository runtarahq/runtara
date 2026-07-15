// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agent component invocation lowering for the direct core emitter.
//!
//! Independently-built agent components share no Rust types, so the emitter
//! speaks their lowered WIT ABI by hand. The invoke signature is
//! `invoke(capability-id: string, input: list<u8>) -> result<list<u8>,
//! error-info>` — no out-of-band connection argument. A connection is delivered
//! inside `input` under `_connection`: `emit_agent_connection_input` resolves it
//! (a `connection_ref` wins over the literal, id-only) and rewrites the input in
//! place before the call, uniformly for every agent kind (primary, memory,
//! MCP-tool). Capability-id and input `(ptr, len)` are pushed directly; the ≤16
//! flat params never spill to the indirect args form.

use wasm_encoder::{Function as WasmFunction, Instruction};

use super::abi::{push_retptr_arg, push_segment_args, push_zero_value};
use super::agent_io::emit_agent_connection_input;
use super::{
    DirectAgentInvokeImport, DirectCoreFunctionIndices, DirectCoreStaticData, DirectDataSegment,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_agent_invoke(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    invoke: &DirectAgentInvokeImport,
    capability_id: &DirectDataSegment,
    static_data: &DirectCoreStaticData,
    agent_id: u32,
    input_ptr_local: u32,
    input_len_local: u32,
    // The execution `source` locals of the CURRENT scope — top-level or a
    // per-iteration Split/While scope — against which a resolvable connection is
    // evaluated. Threaded (not a fixed local) so a ref on an Agent nested inside
    // a subgraph resolves against that subgraph's data, not the top level.
    source_ptr_local: u32,
    source_len_local: u32,
) {
    // Inject the connection into the input under `_connection` — the single
    // connection channel. A connectionless agent is a no-op.
    emit_agent_connection_input(
        body,
        indices,
        static_data,
        agent_id,
        input_ptr_local,
        input_len_local,
        source_ptr_local,
        source_len_local,
    );

    // invoke(capability-id, input): push cap `(ptr, len)` then input `(ptr,
    // len)`. Any trailing lowered params (none for this signature) zero-fill;
    // the last param is the return pointer.
    push_segment_args(body, capability_id);
    body.instruction(&Instruction::LocalGet(input_ptr_local));
    body.instruction(&Instruction::LocalGet(input_len_local));
    for param_type in invoke
        .params
        .get(4..invoke.params.len().saturating_sub(1))
        .unwrap_or(&[])
    {
        push_zero_value(body, param_type);
    }
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(invoke.function_index));
}
