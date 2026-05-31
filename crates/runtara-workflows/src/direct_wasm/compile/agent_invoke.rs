// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agent component invocation argument lowering for the direct core emitter.

use wasm_encoder::{Function as WasmFunction, Instruction};
use wit_parser::abi::WasmType;

use super::abi::{
    push_retptr_arg, push_segment_args, push_zero_value, store_i32_at, store_local_i32_at,
};
use super::{
    DIRECT_AGENT_ARG_CONNECTION_ID_LEN_OFFSET, DIRECT_AGENT_ARG_CONNECTION_ID_PTR_OFFSET,
    DIRECT_AGENT_ARG_CONNECTION_INTEGRATION_LEN_OFFSET,
    DIRECT_AGENT_ARG_CONNECTION_INTEGRATION_PTR_OFFSET,
    DIRECT_AGENT_ARG_CONNECTION_PARAMETERS_LEN_OFFSET,
    DIRECT_AGENT_ARG_CONNECTION_PARAMETERS_PTR_OFFSET,
    DIRECT_AGENT_ARG_CONNECTION_RATE_LIMIT_TAG_OFFSET,
    DIRECT_AGENT_ARG_CONNECTION_SUBTYPE_TAG_OFFSET, DIRECT_AGENT_ARG_CONNECTION_TAG_OFFSET,
    DIRECT_AGENT_ARGS_OFFSET, DirectAgentInvokeImport, DirectCoreStaticData, DirectDataSegment,
};

pub(super) fn emit_agent_invoke(
    body: &mut WasmFunction,
    invoke: &DirectAgentInvokeImport,
    capability_id: &DirectDataSegment,
    static_data: &DirectCoreStaticData,
    agent_id: u32,
    input_ptr_local: u32,
    input_len_local: u32,
) {
    if invoke.params == [WasmType::Pointer, WasmType::Pointer] {
        store_i32_at(body, DIRECT_AGENT_ARGS_OFFSET, capability_id.offset);
        store_i32_at(body, DIRECT_AGENT_ARGS_OFFSET + 4, capability_id.len_i32());
        store_local_i32_at(body, DIRECT_AGENT_ARGS_OFFSET + 8, input_ptr_local);
        store_local_i32_at(body, DIRECT_AGENT_ARGS_OFFSET + 12, input_len_local);
        emit_agent_connection_args(body, static_data, agent_id);
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

fn emit_agent_connection_args(
    body: &mut WasmFunction,
    static_data: &DirectCoreStaticData,
    agent_id: u32,
) {
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
    let (integration_ptr, integration_len) = static_data
        .agent_integration_id(agent_id)
        .map(|segment| (segment.offset, segment.len_i32()))
        .unwrap_or((static_data.agent_empty_integration_id.offset, 0));
    store_i32_at(
        body,
        DIRECT_AGENT_ARG_CONNECTION_INTEGRATION_PTR_OFFSET,
        integration_ptr,
    );
    store_i32_at(
        body,
        DIRECT_AGENT_ARG_CONNECTION_INTEGRATION_LEN_OFFSET,
        integration_len,
    );
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
