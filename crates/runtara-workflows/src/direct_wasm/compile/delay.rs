// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Delay step lowering for the direct workflow core Wasm emitter.
//!
//! A thin step whose only real choice is durable vs. blocking sleep. The duration
//! is computed in the stdlib (`stdlib_delay_duration_ms`) from the resolved
//! source; a durable delay sleeps via `runtime_durable_sleep_checkpoint` keyed by
//! step id so a resumed instance skips an already-elapsed sleep, a non-durable one
//! blocks. Everything else is the usual build-output / rebuild-source /
//! continue-to-next tail.

use wasm_encoder::{Function as WasmFunction, Instruction};

use super::abi::{
    emit_entry_suspend_at, emit_retptr_error_or_step_fail, load_retptr_list, push_retptr_arg,
    push_retptr_i64_load, push_segment_args, return_if_retptr_error, store_local_i64_at,
};
use super::checkpoint::{emit_checkpoint_lookup, emit_checkpoint_save};
use super::debug::{emit_step_breakpoint, emit_step_debug_event};
use super::dispatcher::emit_run_plan_mapping;
use super::mapping::emit_build_source;
use super::{
    DIRECT_DELAY_DURATION_MS_LOCAL, DIRECT_RET_U64_OK_OFFSET, DIRECT_WAIT_DEADLINE_MS_LOCAL,
    DIRECT_WAIT_DEADLINE_SCRATCH_OFFSET, DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL,
    DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL, DirectCoreFunctionIndices, DirectCoreStaticData,
    DirectDataSegment, DirectFailureTarget, DirectHandledTarget, DirectRunPlan, DirectVariables,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_delay_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    delay_id: u32,
    durable: bool,
    breakpoint: bool,
    next_plan: &DirectRunPlan,
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

    body.instruction(&Instruction::I32Const(delay_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_delay_duration_ms));
    // Attribute an unresolvable duration (e.g. a template error) to this step and
    // fail, instead of the bare `return_if_retptr_error` silent exit.
    emit_retptr_error_or_step_fail(
        body,
        indices,
        static_data,
        track_events,
        failure_target,
        step_id,
        source_ptr_local,
        source_len_local,
        route_ptr_local,
        route_len_local,
        output_ptr_local,
        output_len_local,
    );
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_DELAY_DURATION_MS_LOCAL));

    if durable {
        let step_id_segment = static_data
            .step_id(step_id)
            .expect("run plan step ids are present in static data");
        // Per-scope sleep-checkpoint key: bare step id at top level
        // (byte-identical to the legacy static key), `{step}::{indices}`
        // inside Split/While iterations — without the fold, per-item durable
        // delays collide on ONE key (the hazard the unify plan flagged).
        // Stash it in the wait signal-id locals (Delay and WaitForSignal are
        // mutually-exclusive step types, so their scratch is disjoint in time).
        push_segment_args(body, step_id_segment);
        body.instruction(&Instruction::LocalGet(source_ptr_local));
        body.instruction(&Instruction::LocalGet(source_len_local));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_delay_sleep_key));
        return_if_retptr_error(body, indices);
        load_retptr_list(
            body,
            DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL,
            DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL,
        );

        // Store-freeing suspend is opt-in per the unify plan: the DEFAULT under
        // both ABIs remains the legacy blocking `durable-sleep-checkpoint`, whose
        // output is byte-preserved and battery-green. Only when the compile-time
        // gate is set AND we're on the invoke export (whose `outcome::suspended`
        // arm can carry a wake) do we exit-and-reschedule instead of blocking.
        // Blocking is also the right call for short delays — suspend/relaunch
        // would pessimize them — so a future threshold policy lives behind the
        // same gate.
        let store_freeing = indices.store_freeing_sleep
            && indices.abi == crate::direct_wasm::component::WorkflowAbi::InvokeHostImports;
        if !store_freeing {
            // Legacy: block in the host on `durable-sleep-checkpoint`.
            body.instruction(&Instruction::LocalGet(DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL));
            body.instruction(&Instruction::LocalGet(DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL));
            body.instruction(&Instruction::I32Const(0));
            body.instruction(&Instruction::I32Const(0));
            body.instruction(&Instruction::LocalGet(DIRECT_DELAY_DURATION_MS_LOCAL));
            push_retptr_arg(body);
            body.instruction(&Instruction::Call(indices.runtime_durable_sleep_checkpoint));
            return_if_retptr_error(body, indices);
        } else {
            // Store-freeing suspend: the deadline is a durable checkpoint, and
            // the run EXITS with `suspended(at(deadline))` so the host tears
            // down the Store and reschedules a relaunch at the deadline. On
            // resume the replay re-reaches this delay, the checkpoint HITS, and
            // we skip. The deadline is stored, not the remaining duration, so a
            // resume never re-sleeps time that already elapsed.
            emit_checkpoint_lookup(
                body,
                indices,
                DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL,
                DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL,
                output_ptr_local,
                output_len_local,
            );
            // HIT: the host already waited — fall through, skip the sleep.
            body.instruction(&Instruction::Else);
            // MISS: deadline = now + duration; checkpoint it; suspend.
            push_retptr_arg(body);
            body.instruction(&Instruction::Call(indices.runtime_now_ms));
            return_if_retptr_error(body, indices);
            push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
            body.instruction(&Instruction::LocalGet(DIRECT_DELAY_DURATION_MS_LOCAL));
            body.instruction(&Instruction::I64Add);
            body.instruction(&Instruction::LocalSet(DIRECT_WAIT_DEADLINE_MS_LOCAL));
            store_local_i64_at(
                body,
                DIRECT_WAIT_DEADLINE_SCRATCH_OFFSET,
                DIRECT_WAIT_DEADLINE_MS_LOCAL,
            );
            body.instruction(&Instruction::I32Const(DIRECT_WAIT_DEADLINE_SCRATCH_OFFSET));
            body.instruction(&Instruction::LocalSet(output_ptr_local));
            body.instruction(&Instruction::I32Const(8));
            body.instruction(&Instruction::LocalSet(output_len_local));
            emit_checkpoint_save(
                body,
                indices,
                DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL,
                DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL,
                output_ptr_local,
                output_len_local,
            );
            emit_entry_suspend_at(body, DIRECT_WAIT_DEADLINE_MS_LOCAL);
            body.instruction(&Instruction::End);
        }
    } else {
        body.instruction(&Instruction::LocalGet(DIRECT_DELAY_DURATION_MS_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.runtime_blocking_sleep));
        return_if_retptr_error(body, indices);
    }

    body.instruction(&Instruction::I32Const(delay_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    body.instruction(&Instruction::LocalGet(DIRECT_DELAY_DURATION_MS_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_delay));
    return_if_retptr_error(body, indices);
    load_retptr_list(body, steps_ptr_local, steps_len_local);

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
