// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! While step lowering for the direct workflow core Wasm emitter.
//!
//! Repeatedly evaluates a condition and runs the nested subgraph until it is
//! false, max-iterations is hit, the timeout expires, or the instance is
//! cancelled. Like Split, it composes a long-running loop with onError capture,
//! timeout, and durability — all adding block nesting — so it uses the same
//! `DirectFailureTarget` depth-offset discipline, frame spilling, and step-error
//! capture pattern. Timeout and cancellation are enforced per-iteration with early
//! returns (not delegated to the host), which is what keeps an unbounded loop
//! durably interruptible.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    emit_retptr_error_or_return, emit_retptr_error_or_step_fail, load_retptr_list, push_retptr_arg,
    push_retptr_i32_load, push_retptr_i64_load, push_retptr_u8_load, push_variables_args,
};
use super::agent_error::emit_agent_error_route_or_fail;
use super::debug::{emit_step_breakpoint, emit_step_debug_event};
use super::dispatcher::emit_run_plan_mapping;
use super::mapping::emit_build_source;
use super::split::{
    emit_loop_iteration_heap_reset, emit_split_append_error_payload_and_continue,
    emit_value_store_retain,
};
use super::step_error::{pop_step_error_frame, push_step_error_frame};
use super::{
    DIRECT_RET_BOOL_OK_OFFSET, DIRECT_RET_U32_OK_OFFSET, DIRECT_RET_U64_OK_OFFSET,
    DIRECT_STEP_ERROR_FLAG_LOCAL, DIRECT_STEP_ERROR_LEN_LOCAL, DIRECT_STEP_ERROR_PTR_LOCAL,
    DIRECT_WHILE_DEADLINE_MS_LOCAL, DIRECT_WHILE_HEAP_BASE_LOCAL, DIRECT_WHILE_INDEX_LOCAL,
    DIRECT_WHILE_MAX_ITERATIONS_LOCAL, DIRECT_WHILE_PARENT_SOURCE_LEN_LOCAL,
    DIRECT_WHILE_PARENT_SOURCE_PTR_LOCAL, DIRECT_WHILE_PARENT_STEPS_LEN_LOCAL,
    DIRECT_WHILE_PARENT_STEPS_PTR_LOCAL, DIRECT_WHILE_STATE_LEN_LOCAL,
    DIRECT_WHILE_STATE_PTR_LOCAL, DIRECT_WHILE_VARIABLES_LEN_LOCAL,
    DIRECT_WHILE_VARIABLES_PTR_LOCAL, DirectCoreFunctionIndices, DirectCoreStaticData,
    DirectDataSegment, DirectErrorRoutePlan, DirectFailureTarget, DirectHandledTarget,
    DirectRunPlan, DirectVariables, emit_runtime_fail_return,
};

fn push_while_frame(body: &mut WasmFunction) {
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_MAX_ITERATIONS_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_INDEX_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_VARIABLES_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_VARIABLES_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_DEADLINE_MS_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_HEAP_BASE_LOCAL));
}

fn pop_while_frame(body: &mut WasmFunction) {
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_HEAP_BASE_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_DEADLINE_MS_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_VARIABLES_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_VARIABLES_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_STATE_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_INDEX_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_MAX_ITERATIONS_LOCAL));
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_while_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    while_id: u32,
    breakpoint: bool,
    nested_plan: &DirectRunPlan,
    next_plan: &DirectRunPlan,
    error_plan: Option<&DirectErrorRoutePlan>,
    timeout_ms: Option<u64>,
    data_ptr_local: u32,
    data_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
    handled_target: Option<DirectHandledTarget>,
) {
    emit_step_breakpoint(
        body,
        indices,
        static_data,
        breakpoint,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
    );

    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        true,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );

    // When the While step has an onError route, the generated Rust path wraps the
    // whole step in onError handling: any failure inside the loop is captured,
    // injected as `steps.__error`/`steps.error` against the *parent* steps context,
    // and routed to the handler. The direct lowering mirrors that by running the
    // loop inside a capture block whose failures branch to the shared step-error
    // locals, then restoring the parent steps context and routing the captured
    // error after the loop. Lifecycle suspension (cancel/pause/shutdown) still
    // returns early without routing, matching the existing While durability path.
    let has_error_plan = error_plan.is_some();
    let internal_failure_target = if has_error_plan {
        Some(DirectFailureTarget::StepError { branch_depth: 0 })
    } else {
        failure_target
    };

    if has_error_plan {
        body.instruction(&Instruction::LocalGet(steps_ptr_local));
        body.instruction(&Instruction::LocalSet(DIRECT_WHILE_PARENT_STEPS_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(steps_len_local));
        body.instruction(&Instruction::LocalSet(DIRECT_WHILE_PARENT_STEPS_LEN_LOCAL));
        body.instruction(&Instruction::I32Const(0));
        body.instruction(&Instruction::LocalSet(DIRECT_STEP_ERROR_FLAG_LOCAL));
    }

    push_while_frame(body);
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(source_len_local));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_PARENT_SOURCE_LEN_LOCAL));

    if has_error_plan {
        body.instruction(&Instruction::Block(BlockType::Empty));
    }

    body.instruction(&Instruction::I32Const(while_id as i32));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_max_iterations));
    emit_retptr_error_or_return(
        body,
        indices,
        internal_failure_target,
        route_ptr_local,
        route_len_local,
    );
    push_retptr_i32_load(body, DIRECT_RET_U32_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_MAX_ITERATIONS_LOCAL));

    body.instruction(&Instruction::I32Const(while_id as i32));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_initial_state));
    emit_retptr_error_or_return(
        body,
        indices,
        internal_failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(
        body,
        DIRECT_WHILE_STATE_PTR_LOCAL,
        DIRECT_WHILE_STATE_LEN_LOCAL,
    );

    // Capture the heap watermark just above the loop state (the only heap survivor
    // across iterations). Per-iteration scratch (condition source, iteration
    // variables, rebuilt source, step outputs) is bump-allocated above this and
    // reclaimed at the top of each pass; see `emit_loop_iteration_heap_reset`.
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_HEAP_BASE_LOCAL));

    // Resolve the timeout deadline once, before the loop. `timeout_ms` is a static
    // config value; generated Rust parses but does not enforce it, so direct mode
    // is the first to honor the documented "if exceeded, step fails" contract.
    // The deadline is part of the While frame, so nested loops cannot clobber it.
    if let Some(timeout_ms) = timeout_ms {
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.runtime_now_ms));
        emit_retptr_error_or_return(
            body,
            indices,
            internal_failure_target,
            route_ptr_local,
            route_len_local,
        );
        push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
        body.instruction(&Instruction::I64Const(timeout_ms as i64));
        body.instruction(&Instruction::I64Add);
        body.instruction(&Instruction::LocalSet(DIRECT_WHILE_DEADLINE_MS_LOCAL));
    }

    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_INDEX_LOCAL));
    body.instruction(&Instruction::Block(BlockType::Empty));
    body.instruction(&Instruction::Loop(BlockType::Empty));
    let loop_failure_target = internal_failure_target.map(|target| target.nested(2));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_INDEX_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_MAX_ITERATIONS_LOCAL));
    body.instruction(&Instruction::I32GeU);
    body.instruction(&Instruction::BrIf(1));

    // Reclaim the previous iteration's scratch: compact the loop state back down
    // to the watermark and rewind the bump pointer. The condition-false and
    // cancellation exits leave the loop without returning here, so their final
    // pass's scratch is not reclaimed (one iteration's worth — negligible); every
    // continuing pass funnels through this point.
    emit_loop_iteration_heap_reset(
        body,
        DIRECT_WHILE_HEAP_BASE_LOCAL,
        DIRECT_WHILE_STATE_PTR_LOCAL,
        DIRECT_WHILE_STATE_LEN_LOCAL,
    );
    // Reclaim superseded interned values from the host arena, keeping those still
    // reachable from the parent source and the surviving loop state.
    emit_value_store_retain(
        body,
        indices,
        DIRECT_WHILE_PARENT_SOURCE_PTR_LOCAL,
        DIRECT_WHILE_PARENT_SOURCE_LEN_LOCAL,
        DIRECT_WHILE_STATE_PTR_LOCAL,
        DIRECT_WHILE_STATE_LEN_LOCAL,
    );

    // Enforce the wall-clock timeout before each iteration. On expiry the While
    // step fails with the static WHILE_TIMEOUT payload, routed through the same
    // failure target as any other in-loop failure: an onError handler when
    // present, otherwise the enclosing aggregation or `runtime.fail`.
    if timeout_ms.is_some() {
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.runtime_now_ms));
        emit_retptr_error_or_return(
            body,
            indices,
            loop_failure_target,
            route_ptr_local,
            route_len_local,
        );
        push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
        body.instruction(&Instruction::LocalGet(DIRECT_WHILE_DEADLINE_MS_LOCAL));
        body.instruction(&Instruction::I64GeU);
        body.instruction(&Instruction::If(BlockType::Empty));
        body.instruction(&Instruction::I32Const(
            static_data.while_timeout_error.offset,
        ));
        body.instruction(&Instruction::LocalSet(output_ptr_local));
        body.instruction(&Instruction::I32Const(
            static_data.while_timeout_error.len_i32(),
        ));
        body.instruction(&Instruction::LocalSet(output_len_local));
        if let Some(timeout_failure_target) = loop_failure_target {
            emit_split_append_error_payload_and_continue(
                body,
                indices,
                timeout_failure_target.nested(1),
                output_ptr_local,
                output_len_local,
            );
        } else {
            emit_runtime_fail_return(body, indices, output_ptr_local, output_len_local);
        }
        body.instruction(&Instruction::End);
    }

    body.instruction(&Instruction::I32Const(while_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_condition_source));
    emit_retptr_error_or_return(
        body,
        indices,
        loop_failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(body, source_ptr_local, source_len_local);

    body.instruction(&Instruction::I32Const(while_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_condition));
    emit_retptr_error_or_step_fail(
        body,
        indices,
        static_data,
        track_events,
        loop_failure_target,
        step_id,
        source_ptr_local,
        source_len_local,
        route_ptr_local,
        route_len_local,
        output_ptr_local,
        output_len_local,
    );
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::BrIf(1));

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_is_cancelled));
    emit_retptr_error_or_return(
        body,
        indices,
        loop_failure_target,
        route_ptr_local,
        route_len_local,
    );
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    // Suspend-and-exit: ABI-aware (clean-run tag vs suspended outcome).
    super::abi::emit_entry_suspend_return(body, indices);
    body.instruction(&Instruction::End);

    body.instruction(&Instruction::I32Const(while_id as i32));
    push_variables_args(body, variables);
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_iteration_variables));
    emit_retptr_error_or_return(
        body,
        indices,
        loop_failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(
        body,
        DIRECT_WHILE_VARIABLES_PTR_LOCAL,
        DIRECT_WHILE_VARIABLES_LEN_LOCAL,
    );

    body.instruction(&Instruction::I32Const(static_data.steps.offset));
    body.instruction(&Instruction::LocalSet(steps_ptr_local));
    body.instruction(&Instruction::I32Const(static_data.steps.len_i32()));
    body.instruction(&Instruction::LocalSet(steps_len_local));

    let iteration_variables = DirectVariables::Locals {
        ptr_local: DIRECT_WHILE_VARIABLES_PTR_LOCAL,
        len_local: DIRECT_WHILE_VARIABLES_LEN_LOCAL,
    };
    emit_build_source(
        body,
        indices,
        iteration_variables,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        loop_failure_target,
    );

    push_while_frame(body);
    // A handled onError inside the iteration body must continue the loop, not
    // complete the workflow, so wrap the body in a block and give it a depth-0
    // handled target (mirroring the Split body lowering). The step-error frame is
    // saved/restored around the body so a nested While onError capture — which
    // shares the same step-error locals — cannot leak its handled flag into this
    // loop's post-iteration error check. On a real body failure the branch to the
    // capture block skips the restore, leaving the flag set for this loop.
    if has_error_plan {
        push_step_error_frame(body);
        body.instruction(&Instruction::Block(BlockType::Empty));
    }
    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        iteration_variables,
        nested_plan,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
        if has_error_plan {
            loop_failure_target.map(|target| target.nested(1))
        } else {
            loop_failure_target
        },
        if has_error_plan {
            Some(DirectHandledTarget { branch_depth: 0 })
        } else {
            None
        },
    );
    if has_error_plan {
        body.instruction(&Instruction::End);
        pop_step_error_frame(body);
    }
    pop_while_frame(body);

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_heartbeat));
    emit_retptr_error_or_return(
        body,
        indices,
        loop_failure_target,
        route_ptr_local,
        route_len_local,
    );

    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_check_signals));
    emit_retptr_error_or_return(
        body,
        indices,
        loop_failure_target,
        route_ptr_local,
        route_len_local,
    );
    push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    // Suspend-and-exit: ABI-aware (clean-run tag vs suspended outcome).
    super::abi::emit_entry_suspend_return(body, indices);
    body.instruction(&Instruction::End);

    body.instruction(&Instruction::I32Const(while_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_advance_state));
    emit_retptr_error_or_return(
        body,
        indices,
        loop_failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(
        body,
        DIRECT_WHILE_STATE_PTR_LOCAL,
        DIRECT_WHILE_STATE_LEN_LOCAL,
    );

    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_INDEX_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_WHILE_INDEX_LOCAL));
    body.instruction(&Instruction::Br(0));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);

    body.instruction(&Instruction::I32Const(while_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WHILE_STATE_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_while_output));
    emit_retptr_error_or_return(
        body,
        indices,
        internal_failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(body, steps_ptr_local, steps_len_local);

    if has_error_plan {
        body.instruction(&Instruction::End);
    }

    pop_while_frame(body);

    if let Some(error_plan) = error_plan {
        // A failure inside the loop set the step-error flag and branched to the
        // capture block end. Restore the parent steps context and route the
        // captured error through the shared onError machinery.
        body.instruction(&Instruction::LocalGet(DIRECT_STEP_ERROR_FLAG_LOCAL));
        body.instruction(&Instruction::If(BlockType::Empty));
        body.instruction(&Instruction::LocalGet(DIRECT_WHILE_PARENT_STEPS_PTR_LOCAL));
        body.instruction(&Instruction::LocalSet(steps_ptr_local));
        body.instruction(&Instruction::LocalGet(DIRECT_WHILE_PARENT_STEPS_LEN_LOCAL));
        body.instruction(&Instruction::LocalSet(steps_len_local));
        emit_agent_error_route_or_fail(
            body,
            indices,
            static_data,
            track_events,
            variables,
            step_id,
            DIRECT_STEP_ERROR_PTR_LOCAL,
            DIRECT_STEP_ERROR_LEN_LOCAL,
            steps_ptr_local,
            steps_len_local,
            source_ptr_local,
            source_len_local,
            output_ptr_local,
            output_len_local,
            route_ptr_local,
            route_len_local,
            Some(error_plan),
            data_ptr_local,
            data_len_local,
            workflow_log_kind,
            workflow_error_kind,
            failure_target.map(|target| target.nested(1)),
            handled_target.map(|target| target.nested(1)),
        );
        body.instruction(&Instruction::End);
    }

    emit_build_source(
        body,
        indices,
        variables,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        failure_target,
    );

    emit_step_debug_event(
        body,
        indices,
        static_data,
        track_events,
        false,
        step_id,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );

    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        variables,
        next_plan,
        data_ptr_local,
        data_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target,
        handled_target,
    );
}
