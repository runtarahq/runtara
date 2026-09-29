// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! WaitForInstances lowering for the direct workflow core emitter.
//!
//! The step parks its run on a durable instance wait
//! (`runtara:workflow/waits`) until direct child runs finish:
//!
//! 1. Breakpoint, then the step's canonical v2 `wait-instances` key
//!    (`stdlib.wait-instances-key`). The host hashes it into the wait id; the
//!    settled wait is checkpointed under it.
//! 2. A checkpoint HIT is the settled wait of an earlier pass: it becomes the
//!    output without touching the wait again.
//! 3. On a MISS the stdlib builds the request (resolved `instanceIds`, `mode`
//!    and, with `timeoutMs`, `now + timeoutMs`) and `instances.register`
//!    registers it, or finds it on replay, and reads it. The host keeps the
//!    first registration's deadline, so the deadline needs no checkpoint of
//!    its own: every later pass sends a later one and reads back the first.
//! 4. A pending read parks the run: `suspended([at(deadline)])` with the
//!    persisted deadline (clamped by an enclosing loop deadline), or
//!    `suspended([on-resume])` without one. The host attaches the pending
//!    wait out of band (`InvokeRunResult::instance_waits`), so the runner
//!    parks on it. The relaunch replays to
//!    this step and registers again, which now reads the settled wait.
//! 5. A settled read is checkpointed, the wait is released (its row deleted)
//!    and the ordinary output / source / debug / next-plan tail runs.
//!
//! A failed request or registration is the step error `INSTANCE_WAIT_<CODE>`
//! (`stdlib.wait-instances-error`), routed to the step's onError handler or
//! the enclosing failure target, else failing the run. The host already closed
//! the step's wait, so a retry of an enclosing region registers afresh.
//!
//! Only a workflow with the host-imported runtime reaches this lowering;
//! `agent_suspend::check_sites` refuses every other target.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    emit_call_wide_result, emit_entry_suspend_at, emit_entry_suspend_return,
    emit_retptr_error_or_return, emit_retptr_error_or_step_fail, load_retptr_list, load_retptr_tag,
    push_retptr_arg, push_retptr_i64_load, push_retptr_u8_load, push_segment_args,
    return_if_retptr_error,
};
use super::agent_error::emit_agent_error_route_or_fail;
use super::checkpoint::{emit_checkpoint, emit_checkpoint_lookup};
use super::debug::{emit_step_breakpoint, emit_step_debug_error, emit_step_debug_event};
use super::dispatcher::emit_run_plan_mapping;
use super::mapping::emit_build_source;
use super::{
    DIRECT_RET_U64_OK_OFFSET, DIRECT_STEP_ERROR_LEN_LOCAL, DIRECT_STEP_ERROR_PTR_LOCAL,
    DIRECT_WAIT_DEADLINE_MS_LOCAL, DIRECT_WAIT_RESUMED_LOCAL, DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL,
    DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL, DIRECT_WAIT_TIMEOUT_PRESENT_LOCAL, DirectCoreFunctionIndices,
    DirectCoreStaticData, DirectDataSegment, DirectErrorRoutePlan, DirectFailureTarget,
    DirectHandledTarget, DirectRunPlan, DirectVariables,
};

// Canonical layout of `result<wait-instances-progress, string>` in the retptr
// area, where `wait-instances-progress = record { pending: bool, deadline-ms:
// option<u64> }`: the 8-aligned record follows the tag at +8, its option at
// record offset 8. `wait_instances_tests` re-derives them from the WIT.
/// `pending`.
pub(super) const PROGRESS_PENDING_OFFSET: u64 = 8;
/// `deadline-ms` option tag.
pub(super) const PROGRESS_DEADLINE_TAG_OFFSET: u64 = 16;
/// `deadline-ms` value.
pub(super) const PROGRESS_DEADLINE_VALUE_OFFSET: u64 = 24;

// Locals. The step key lives in the WaitForSignal signal-id pair and the
// failure flag in its resume flag: the two step types never interleave (a
// WaitForInstances step cannot sit in an onWait graph). The park deadline
// reuses the wait-deadline pair, the error the shared step-error pair.
const KEY_PTR: u32 = DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL;
const KEY_LEN: u32 = DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL;
const FAILED: u32 = DIRECT_WAIT_RESUMED_LOCAL;
const DEADLINE_PRESENT: u32 = DIRECT_WAIT_TIMEOUT_PRESENT_LOCAL;
const DEADLINE: u32 = DIRECT_WAIT_DEADLINE_MS_LOCAL;
const ERROR_PTR: u32 = DIRECT_STEP_ERROR_PTR_LOCAL;
const ERROR_LEN: u32 = DIRECT_STEP_ERROR_LEN_LOCAL;

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_wait_for_instances_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    breakpoint: bool,
    next_plan: &DirectRunPlan,
    error_plan: Option<&DirectErrorRoutePlan>,
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
    let step_id_segment = static_data
        .step_id(step_id)
        .expect("run plan step ids are present in static data");
    let waits = indices.wait_instances().clone();

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

    // key = stdlib.wait-instances-key(step, source)
    push_segment_args(body, step_id_segment);
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_wait_instances_key));
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
    load_retptr_list(body, KEY_PTR, KEY_LEN);

    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(FAILED));

    // HIT: the settled wait of an earlier pass is the output.
    emit_checkpoint_lookup(
        body,
        indices,
        KEY_PTR,
        KEY_LEN,
        output_ptr_local,
        output_len_local,
    );
    body.instruction(&Instruction::Else);
    emit_register(
        body,
        indices,
        &waits,
        step_id_segment,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
    );
    body.instruction(&Instruction::End);

    // A failed request or registration, as the step's structured error.
    body.instruction(&Instruction::LocalGet(FAILED));
    body.instruction(&Instruction::If(BlockType::Empty));
    push_segment_args(body, step_id_segment);
    body.instruction(&Instruction::LocalGet(ERROR_PTR));
    body.instruction(&Instruction::LocalGet(ERROR_LEN));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_wait_instances_error));
    return_if_retptr_error(body, indices);
    load_retptr_list(body, ERROR_PTR, ERROR_LEN);
    emit_step_debug_error(
        body,
        indices,
        static_data,
        track_events,
        step_id,
        source_ptr_local,
        source_len_local,
        ERROR_PTR,
        ERROR_LEN,
        route_ptr_local,
        route_len_local,
    );
    // The steps context is still the parent's: the step stored nothing yet.
    emit_agent_error_route_or_fail(
        body,
        indices,
        static_data,
        track_events,
        variables,
        step_id,
        ERROR_PTR,
        ERROR_LEN,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        output_ptr_local,
        output_len_local,
        route_ptr_local,
        route_len_local,
        error_plan,
        data_ptr_local,
        data_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target.map(|target| target.nested(1)),
        handled_target.map(|target| target.nested(1)),
    );
    body.instruction(&Instruction::End);

    // steps = stdlib.wait-instances-output(step, wait, source)
    push_segment_args(body, step_id_segment);
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_wait_instances_output));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        output_ptr_local,
        output_len_local,
    );
    load_retptr_list(body, steps_ptr_local, steps_len_local);

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

/// The MISS arm: build the request, register (or find) the wait and read it.
/// Leaves a settled wait in `output_*` (checkpointed and released), parks on
/// a pending one, or sets `FAILED` with the wait error in the error pair.
#[allow(clippy::too_many_arguments)]
fn emit_register(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    waits: &super::core_imports::DirectWaitInstancesImports,
    step_id_segment: &DirectDataSegment,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    // request = stdlib.wait-instances-request(step, source, now)
    push_retptr_arg(body);
    emit_call_wide_result(body, indices.runtime_now_ms);
    return_if_retptr_error(body, indices);
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DEADLINE));
    push_segment_args(body, step_id_segment);
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    body.instruction(&Instruction::LocalGet(DEADLINE));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_wait_instances_request));
    fail_if_retptr_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);

    // wait = instances.register(key, request)
    body.instruction(&Instruction::LocalGet(KEY_PTR));
    body.instruction(&Instruction::LocalGet(KEY_LEN));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(waits.register));
    fail_if_retptr_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);

    // Pending: park, on the persisted deadline when the wait has one.
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    emit_call_wide_result(body, indices.stdlib_wait_instances_state);
    return_if_retptr_error(body, indices);
    push_retptr_u8_load(body, PROGRESS_PENDING_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    push_retptr_u8_load(body, PROGRESS_DEADLINE_TAG_OFFSET);
    body.instruction(&Instruction::LocalSet(DEADLINE_PRESENT));
    push_retptr_i64_load(body, PROGRESS_DEADLINE_VALUE_OFFSET);
    body.instruction(&Instruction::LocalSet(DEADLINE));
    emit_park(body, indices);
    body.instruction(&Instruction::End);

    // Settled: checkpoint the wait, then drop its row. The release returns
    // nothing, so the checkpoint's signal result is still in the retptr area
    // for the signal handling that follows.
    body.instruction(&Instruction::LocalGet(KEY_PTR));
    body.instruction(&Instruction::LocalGet(KEY_LEN));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    emit_checkpoint(body, indices);
    body.instruction(&Instruction::LocalGet(KEY_PTR));
    body.instruction(&Instruction::LocalGet(KEY_LEN));
    body.instruction(&Instruction::Call(waits.release));
    super::cooperative_wait::emit_checkpoint_signal(body, indices);

    // Close the two `else` arms fail_if_retptr_error opened.
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
}

/// On an error result, move its string to the error pair and set `FAILED`;
/// otherwise continue in an `else` arm the caller closes.
fn fail_if_retptr_error(body: &mut WasmFunction) {
    load_retptr_tag(body);
    body.instruction(&Instruction::If(BlockType::Empty));
    load_retptr_list(body, ERROR_PTR, ERROR_LEN);
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::LocalSet(FAILED));
    body.instruction(&Instruction::Else);
}

/// Exit `suspended([at(deadline)])` when a deadline is present (after the
/// enclosing loop deadline clamped it), else `suspended([on-resume])`; the
/// pending wait the host recorded is what wakes the run.
fn emit_park(body: &mut WasmFunction, indices: &DirectCoreFunctionIndices) {
    super::loop_deadline::clamp(body, DEADLINE, Some(DEADLINE_PRESENT));
    body.instruction(&Instruction::LocalGet(DEADLINE_PRESENT));
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_entry_suspend_at(body, indices, DEADLINE);
    body.instruction(&Instruction::Else);
    emit_entry_suspend_return(body, indices);
    body.instruction(&Instruction::End);
}
