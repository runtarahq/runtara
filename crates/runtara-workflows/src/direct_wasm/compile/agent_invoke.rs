// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agent component invocation argument lowering for the direct core emitter.
//!
//! Independently-built agent components share no Rust types, so the emitter speaks
//! their lowered WIT ABI by hand. `emit_agent_invoke` handles both call shapes:
//! the canonical two-pointer form writes the capability-id and input `(ptr, len)`
//! plus a tagged connection sub-struct into a fixed args struct in linear memory
//! and calls with a pointer to it; other shapes push the args directly on the
//! stack. Connection metadata is written as a tagged struct carrying only the
//! connection id (never secrets or provider/integration metadata).

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};
use wit_parser::abi::WasmType;

use super::abi::{
    emit_fail_if_retptr_error_inplace, load_retptr_list, push_retptr_arg, push_segment_args,
    push_zero_value, store_i32_at, store_local_i32_at,
};
use super::{
    DIRECT_AGENT_ARG_CONNECTION_ID_LEN_OFFSET, DIRECT_AGENT_ARG_CONNECTION_ID_PTR_OFFSET,
    DIRECT_AGENT_ARG_CONNECTION_INTEGRATION_LEN_OFFSET,
    DIRECT_AGENT_ARG_CONNECTION_INTEGRATION_PTR_OFFSET,
    DIRECT_AGENT_ARG_CONNECTION_PARAMETERS_LEN_OFFSET,
    DIRECT_AGENT_ARG_CONNECTION_PARAMETERS_PTR_OFFSET,
    DIRECT_AGENT_ARG_CONNECTION_RATE_LIMIT_TAG_OFFSET,
    DIRECT_AGENT_ARG_CONNECTION_SUBTYPE_TAG_OFFSET, DIRECT_AGENT_ARG_CONNECTION_TAG_OFFSET,
    DIRECT_AGENT_ARGS_OFFSET, DIRECT_AGENT_CONN_ID_LEN_LOCAL, DIRECT_AGENT_CONN_ID_PTR_LOCAL,
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
    if invoke.params == [WasmType::Pointer, WasmType::Pointer] {
        store_i32_at(body, DIRECT_AGENT_ARGS_OFFSET, capability_id.offset);
        store_i32_at(body, DIRECT_AGENT_ARGS_OFFSET + 4, capability_id.len_i32());
        store_local_i32_at(body, DIRECT_AGENT_ARGS_OFFSET + 8, input_ptr_local);
        store_local_i32_at(body, DIRECT_AGENT_ARGS_OFFSET + 12, input_len_local);
        emit_agent_connection_args(
            body,
            indices,
            static_data,
            agent_id,
            source_ptr_local,
            source_len_local,
        );
        body.instruction(&Instruction::I32Const(DIRECT_AGENT_ARGS_OFFSET));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(invoke.function_index));
        return;
    }

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

/// Write the agent-invoke `connection` argument (id-only; the proxy resolves the
/// real integration + credentials server-side, so integration/parameters stay
/// empty). Three cases: a resolvable `connection_ref` is resolved at runtime
/// against the execution source; a literal `connection_id` is read from its baked
/// segment; no connection writes a `none` tag.
fn emit_agent_connection_args(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    agent_id: u32,
    source_ptr_local: u32,
    source_len_local: u32,
) {
    // Resolvable ref: `resolve-connection-id(agent_id, source)` returns the id
    // (empty = none). This is the uniform path that reaches every agent kind —
    // including memory / MCP-tool agents whose input is not a mapping — because
    // the agent-side `invoke` glue merges this WIT `connection` arg into
    // `input._connection`.
    if static_data.agent_has_connection_ref(agent_id) {
        body.instruction(&Instruction::I32Const(agent_id as i32));
        body.instruction(&Instruction::LocalGet(source_ptr_local));
        body.instruction(&Instruction::LocalGet(source_len_local));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_resolve_connection_id));
        emit_fail_if_retptr_error_inplace(body, indices);
        load_retptr_list(
            body,
            DIRECT_AGENT_CONN_ID_PTR_LOCAL,
            DIRECT_AGENT_CONN_ID_LEN_LOCAL,
        );

        // Empty resolved id → `none`; else a `some` connection with the id.
        body.instruction(&Instruction::LocalGet(DIRECT_AGENT_CONN_ID_LEN_LOCAL));
        body.instruction(&Instruction::I32Eqz);
        body.instruction(&Instruction::If(BlockType::Empty));
        store_i32_at(body, DIRECT_AGENT_ARG_CONNECTION_TAG_OFFSET, 0);
        body.instruction(&Instruction::Else);
        store_i32_at(body, DIRECT_AGENT_ARG_CONNECTION_TAG_OFFSET, 1);
        store_local_i32_at(
            body,
            DIRECT_AGENT_ARG_CONNECTION_ID_PTR_OFFSET,
            DIRECT_AGENT_CONN_ID_PTR_LOCAL,
        );
        store_local_i32_at(
            body,
            DIRECT_AGENT_ARG_CONNECTION_ID_LEN_OFFSET,
            DIRECT_AGENT_CONN_ID_LEN_LOCAL,
        );
        emit_connection_metadata_args(body, static_data);
        body.instruction(&Instruction::End);
        return;
    }

    let Some(connection_id) = static_data.agent_connection_id(agent_id) else {
        store_i32_at(body, DIRECT_AGENT_ARG_CONNECTION_TAG_OFFSET, 0);
        return;
    };

    store_i32_at(body, DIRECT_AGENT_ARG_CONNECTION_TAG_OFFSET, 1);
    store_i32_at(
        body,
        DIRECT_AGENT_ARG_CONNECTION_ID_PTR_OFFSET,
        connection_id.offset,
    );
    store_i32_at(
        body,
        DIRECT_AGENT_ARG_CONNECTION_ID_LEN_OFFSET,
        connection_id.len_i32(),
    );
    emit_connection_metadata_args(body, static_data);
}

/// Write the id-only connection metadata (empty integration, empty parameters,
/// no subtype, no rate-limit) shared by the literal and resolved-ref paths.
fn emit_connection_metadata_args(body: &mut WasmFunction, static_data: &DirectCoreStaticData) {
    store_i32_at(
        body,
        DIRECT_AGENT_ARG_CONNECTION_INTEGRATION_PTR_OFFSET,
        static_data.agent_empty_integration_id.offset,
    );
    store_i32_at(body, DIRECT_AGENT_ARG_CONNECTION_INTEGRATION_LEN_OFFSET, 0);
    store_i32_at(body, DIRECT_AGENT_ARG_CONNECTION_SUBTYPE_TAG_OFFSET, 0);
    store_i32_at(
        body,
        DIRECT_AGENT_ARG_CONNECTION_PARAMETERS_PTR_OFFSET,
        static_data.agent_empty_parameters.offset,
    );
    store_i32_at(
        body,
        DIRECT_AGENT_ARG_CONNECTION_PARAMETERS_LEN_OFFSET,
        static_data.agent_empty_parameters.len_i32(),
    );
    store_i32_at(body, DIRECT_AGENT_ARG_CONNECTION_RATE_LIMIT_TAG_OFFSET, 0);
}
