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
//! MCP-tool). The base call's four flat input parameters fit async lowering;
//! retained scoped invocations use the existing indirect-argument shim.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    emit_agent_suspend_sentinel_check, emit_fail_if_retptr_error_inplace, push_retptr_arg,
    push_retptr_i32_load, push_segment_args, push_zero_value,
};
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
    site: AgentInvocationSite,
) {
    super::cooperative_wait::emit_poll_before_call(body, indices);
    let deadline = matches!(site, AgentInvocationSite::Step(_))
        && static_data.agent_timeout(agent_id).is_some();
    let scoped_deadline = indices.monotonic_now.is_some();
    if scoped_deadline {
        super::deadline_scope::choose(body, indices, deadline);
        body.instruction(&Instruction::LocalGet(super::agent_deadline::REMAINING));
        body.instruction(&Instruction::I64Eqz);
        body.instruction(&Instruction::If(BlockType::Empty));
        super::deadline_scope::select(body);
        super::agent_deadline::error(body, static_data);
        body.instruction(&Instruction::Else);
    }
    let async_invoke = indices
        .agent_invokes
        .iter()
        .find(|(_, candidate)| candidate.function_index == invoke.function_index)
        .and_then(|(agent, _)| indices.agent_invokes_async.get(agent))
        .expect("every Agent has a standard async lowering");

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

    if scoped_deadline {
        super::deadline_scope::arm(body, indices, deadline);
    }

    // invoke(capability-id, input): push cap `(ptr, len)` then input `(ptr,
    // len)`. Any trailing lowered params (none for this signature) zero-fill;
    // the last param is the return pointer.
    push_segment_args(body, capability_id);
    body.instruction(&Instruction::LocalGet(input_ptr_local));
    body.instruction(&Instruction::LocalGet(input_len_local));
    if invoke.is_scoped() {
        emit_agent_context(
            body,
            indices,
            static_data,
            agent_id,
            source_ptr_local,
            source_len_local,
            site,
        );
    } else {
        for param_type in invoke
            .params
            .get(4..invoke.params.len().saturating_sub(1))
            .unwrap_or(&[])
        {
            push_zero_value(body, param_type);
        }
    }
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(async_invoke.function_index));
    super::cooperative_wait::emit_await_call(body, indices);
    if scoped_deadline {
        body.instruction(&Instruction::LocalGet(super::cooperative_wait::TIMED_OUT));
        body.instruction(&Instruction::If(BlockType::Empty));
        super::deadline_scope::select(body);
        // Ignore a late result written while subtask.cancel resolved the call.
        super::agent_deadline::error(body, static_data);
        body.instruction(&Instruction::End);
    }

    // A workflow-agent child shares this instance's runtime host, so a
    // lifecycle suspend (pause/shutdown ack) can fire INSIDE the child; the
    // capability channel carries it out as the suspend sentinel error.
    // Re-raise it through our own ABI here — before retry classification,
    // per-attempt checkpointing, or onError routing can misread it as a
    // failure. Native agents never raise the sentinel (and the check is
    // gated off their invokes entirely).
    if static_data.agent_is_workflow_agent(agent_id) {
        emit_agent_suspend_sentinel_check(body, indices);
    }
    if scoped_deadline {
        body.instruction(&Instruction::End);
    }
}

/// Stable domains distinguish auxiliary invocations even when they target the
/// same Agent. AI activation counters are restored by the guest replay logic.
#[derive(Clone, Copy)]
pub(super) enum AgentInvocationSite {
    Step(Option<u32>),
    MemoryLoad,
    AiTurn,
    AiTool(u32),
    Summarize,
    MemorySave,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_agent_context(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    agent_id: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    site: AgentInvocationSite,
) {
    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_cache_key));
    emit_fail_if_retptr_error_inplace(body, indices);
    push_retptr_i32_load(body, 4);
    push_retptr_i32_load(body, 8);
    let (domain, activation) = match site {
        AgentInvocationSite::Step(_) => (0, None),
        AgentInvocationSite::MemoryLoad => (1, None),
        AgentInvocationSite::AiTurn => (2, Some(super::DIRECT_AI_ITER_LOCAL)),
        AgentInvocationSite::AiTool(_) => (3, Some(super::DIRECT_AI_TOOL_CALL_COUNTER_LOCAL)),
        AgentInvocationSite::Summarize => (4, None),
        AgentInvocationSite::MemorySave => (5, None),
    };
    let caller = match site {
        AgentInvocationSite::AiTool(caller) => caller,
        _ => agent_id,
    };
    body.instruction(&Instruction::I32Const(
        static_data.invocation_site(agent_id, caller, domain) as i32,
    ));
    body.instruction(&activation.map_or(Instruction::I32Const(0), Instruction::LocalGet));
    if let AgentInvocationSite::Step(Some(attempt)) = site {
        body.instruction(&Instruction::LocalGet(attempt));
        body.instruction(&Instruction::I64ExtendI32U);
    } else {
        body.instruction(&Instruction::I64Const(1));
    }
}
