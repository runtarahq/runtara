// Copyright (C) 2026 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! AI tools and memory calls share Agent budget initialization and result checkpoints.
//! Their source namespaces are distinct; connection resolution keeps the caller source.
use super::abi::{load_retptr_list, push_retptr_arg, push_segment_args, return_if_retptr_error};
use super::*;
use wasm_encoder::{Function, Instruction};

#[allow(clippy::too_many_arguments)]
pub(super) fn enter_tool(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    data: &DirectCoreStaticData,
    agent: u32,
    step: &str,
    caller: &str,
    label: &str,
    durable: bool,
    timeout: u64,
    source: (u32, u32),
) {
    // Result locals are scratch until invoke/capture, and remain intact on a
    // checkpoint miss. Never replace the caller source used for connections.
    push_segment_args(body, data.step_id(caller).expect("AI step"));
    push_segment_args(body, data.step_id(label).expect("tool label"));
    body.instruction(&Instruction::LocalGet(DIRECT_AI_TOOL_CALL_COUNTER_LOCAL));
    body.instruction(&Instruction::LocalGet(source.0));
    body.instruction(&Instruction::LocalGet(source.1));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_tool_scope_source));
    return_if_retptr_error(body, indices);
    load_retptr_list(
        body,
        DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
        DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
    );
    enter_scoped(body, indices, data, agent, step, durable, timeout);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn enter_memory(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    data: &DirectCoreStaticData,
    agent: u32,
    step: &str,
    durable: bool,
    timeout: u64,
    source: (u32, u32),
) {
    body.instruction(&Instruction::I32Const(agent as i32));
    body.instruction(&Instruction::LocalGet(source.0));
    body.instruction(&Instruction::LocalGet(source.1));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_aux_scope_source));
    return_if_retptr_error(body, indices);
    load_retptr_list(
        body,
        DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
        DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
    );
    enter_scoped(body, indices, data, agent, step, durable, timeout);
}

#[allow(clippy::too_many_arguments)]
fn enter_scoped(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    data: &DirectCoreStaticData,
    agent: u32,
    step: &str,
    durable: bool,
    timeout: u64,
) {
    if durable {
        super::agent_io::emit_agent_cache_key(
            body,
            indices,
            agent,
            DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
            DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
            DIRECT_AGENT_ATTEMPT_KEY_PTR_LOCAL,
            DIRECT_AGENT_ATTEMPT_KEY_LEN_LOCAL,
        );
        super::checkpoint::emit_checkpoint_lookup(
            body,
            indices,
            DIRECT_AGENT_ATTEMPT_KEY_PTR_LOCAL,
            DIRECT_AGENT_ATTEMPT_KEY_LEN_LOCAL,
            DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
            DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
        );
        body.instruction(&Instruction::Else);
    }
    super::agent_deadline::enter(
        body,
        indices,
        &data.agent_deadline_state_error,
        data.step_id(step).expect("Agent definition step"),
        (
            DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
            DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
        ),
        timeout,
        durable,
    );
}

pub(super) fn finish(body: &mut Function, indices: &DirectCoreFunctionIndices, durable: bool) {
    if durable {
        // Parent expiry propagates before this point. Only a completed local
        // result is replayable. Tools capture local errors as model feedback;
        // memory errors take the existing AI error route before this point.
        super::checkpoint::emit_checkpoint_save(
            body,
            indices,
            DIRECT_AGENT_ATTEMPT_KEY_PTR_LOCAL,
            DIRECT_AGENT_ATTEMPT_KEY_LEN_LOCAL,
            DIRECT_AI_TOOL_RESULT_PTR_LOCAL,
            DIRECT_AI_TOOL_RESULT_LEN_LOCAL,
        );
        body.instruction(&Instruction::End);
    }
}
