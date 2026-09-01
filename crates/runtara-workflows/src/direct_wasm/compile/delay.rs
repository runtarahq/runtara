// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Delay step lowering for the direct workflow core Wasm emitter.
//!
//! A thin step whose only real choice is durable vs. blocking sleep. The duration
//! is computed in the stdlib (`stdlib_delay_duration_ms`) from the resolved
//! source; a non-durable delay always blocks.
//!
//! A durable delay under the invoke export PARKS: it checkpoints an absolute
//! deadline and exits with `outcome::suspended(at(deadline))`, so the host frees
//! the wasmtime Store and the wake scheduler relaunches at the deadline —
//! instead of pinning a Store and a tokio task for the whole wait. Short delays
//! still block, chosen against [`DIRECT_DURABLE_DELAY_PARK_THRESHOLD_MS`] at
//! RUNTIME rather than compile time, because `durationMs` may be a reference the
//! emitter cannot resolve. Both arms are therefore emitted into every durable
//! delay under that export.
//!
//! Everything else is the usual build-output / rebuild-source / continue-to-next
//! tail.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    emit_entry_suspend_at, emit_retptr_error_or_step_fail, load_retptr_list,
    push_i64_load_from_ptr, push_retptr_arg, push_retptr_i64_load, push_segment_args,
    return_if_retptr_error, store_local_i64_at,
};
use super::checkpoint::{
    emit_check_signals_and_suspend, emit_checkpoint_lookup, emit_checkpoint_save,
};
use super::debug::{emit_step_breakpoint, emit_step_debug_event};
use super::dispatcher::emit_run_plan_mapping;
use super::mapping::emit_build_source;
use super::{
    DIRECT_DELAY_DURATION_MS_LOCAL, DIRECT_RET_U64_OK_OFFSET, DIRECT_WAIT_DEADLINE_MS_LOCAL,
    DIRECT_WAIT_DEADLINE_SCRATCH_OFFSET, DIRECT_WAIT_ON_WAIT_VARIABLES_LEN_LOCAL,
    DIRECT_WAIT_ON_WAIT_VARIABLES_PTR_LOCAL, DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL,
    DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL, DirectCoreFunctionIndices, DirectCoreStaticData,
    DirectDataSegment, DirectFailureTarget, DirectHandledTarget, DirectRunPlan, DirectVariables,
};

/// Durable-delay park threshold, in milliseconds: at or above it a delay frees
/// the Store and reschedules, below it the delay blocks in the host.
///
/// Parking is not free. It costs a checkpoint write, a Store teardown, up to one
/// wake-scheduler poll interval of lag (5s by default), and a relaunch that
/// REPLAYS the workflow from its entry step — so every already-completed step
/// pays a checkpoint lookup to skip itself, making the cost grow with how deep
/// the delay sits in the graph. Blocking costs one pinned Store for the
/// duration.
///
/// 30s sits comfortably above the poll interval, which is what makes the trade
/// work: scheduler lag stays a small fraction of the wait rather than dwarfing
/// it, and a sub-second pause inside a While loop keeps blocking instead of
/// parking and replaying on every iteration. Delays long enough to matter — the
/// hours-long business waits that pinned a Store for hours — are all far above
/// it.
const DIRECT_DURABLE_DELAY_PARK_THRESHOLD_MS: i64 = 30_000;

/// Width of a park's checkpoint state: one `u64` absolute deadline.
///
/// A park's state is checked against this before it is trusted, because the
/// blocking arm's key is the SAME key and core's `handle_sleep` saves a
/// checkpoint under it with an EMPTY state. `get-checkpoint` reports that as
/// `some([])` — a HIT — so a delay that blocked on one pass and parks on a
/// later replay (its `durationMs` is a reference that resolved differently)
/// would otherwise read the blocking arm's empty state as "already waited" and
/// skip its wait entirely.
const DIRECT_DELAY_DEADLINE_STATE_LEN: i32 = 8;

/// Blocking durable sleep: the host holds the wasmtime Store and the tokio task
/// for the whole duration on `durable-sleep-checkpoint`.
///
/// The host saves the sleep checkpoint but does NOT look one up, so a replayed
/// blocking delay sleeps again — bounded by the park threshold, which is why the
/// threshold has to stay small enough for a re-slept delay to be cheap.
fn emit_blocking_durable_sleep(body: &mut WasmFunction, indices: &DirectCoreFunctionIndices) {
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalGet(DIRECT_DELAY_DURATION_MS_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_durable_sleep_checkpoint));
    return_if_retptr_error(body, indices);
    // The sleep writes no checkpoint on the GUEST side, so it has no
    // `emit_checkpoint_save` to fold signal handling into. Poll explicitly:
    // without this a cancel that arrived mid-sleep is never observed, and a
    // chain of delays has no poll site at all — the run ignores the cancel and
    // finishes normally.
    emit_check_signals_and_suspend(body, indices);
}

/// Store-freeing park: checkpoint an absolute deadline and EXIT with
/// `suspended(at(deadline))`, so the host tears down the Store and the wake
/// scheduler relaunches at the deadline. The DEADLINE is stored, not the
/// remaining duration, so a resume never re-sleeps time that already elapsed.
///
/// A HIT does NOT mean the wait is over — it means a deadline was recorded. The
/// guest re-reads it and compares against `now`, because the wake scheduler is
/// not the only thing that can relaunch a parked instance: `handle_resume_instance`
/// accepts any row whose status is `suspended`, with no `termination_reason`
/// filter, so an operator (or the MCP `resume_execution` tool) resuming a
/// workflow parked on a 24-hour Delay reaches this code early. Treating the
/// checkpoint's mere existence as "already waited" would make that resume skip
/// the entire delay.
///
/// The deadline bytes are routed into wait-only scratch locals rather than the
/// step output locals, so the parking and blocking arms leave identical state
/// behind — the deadline must never be observable as a step output, however the
/// delay is followed.
fn emit_park_until_deadline(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    emit_checkpoint_lookup(
        body,
        indices,
        DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL,
        DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL,
        DIRECT_WAIT_ON_WAIT_VARIABLES_PTR_LOCAL,
        DIRECT_WAIT_ON_WAIT_VARIABLES_LEN_LOCAL,
    );
    // A HIT of exactly one u64 is a deadline THIS arm wrote: re-read it and
    // decide. Any other width is core's `handle_sleep` checkpoint, saved under
    // this same key with an EMPTY state when the blocking arm ran — which means
    // the wait was already served on that pass, so falling through is the
    // correct resume. (It must be a fall-through, not a re-park: `handle_checkpoint`
    // is get-or-SET, so a save under an occupied key is a no-op and a re-park
    // would never persist its deadline — it would park again on every relaunch,
    // forever.)
    body.instruction(&Instruction::LocalGet(
        DIRECT_WAIT_ON_WAIT_VARIABLES_LEN_LOCAL,
    ));
    body.instruction(&Instruction::I32Const(DIRECT_DELAY_DEADLINE_STATE_LEN));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    push_i64_load_from_ptr(body, DIRECT_WAIT_ON_WAIT_VARIABLES_PTR_LOCAL);
    body.instruction(&Instruction::LocalSet(DIRECT_WAIT_DEADLINE_MS_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_now_ms));
    return_if_retptr_error(body, indices);
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::LocalGet(DIRECT_WAIT_DEADLINE_MS_LOCAL));
    // Unsigned: both sides are u64 epoch milliseconds.
    body.instruction(&Instruction::I64LtU);
    body.instruction(&Instruction::If(BlockType::Empty));
    // Woken early — re-park on the SAME absolute deadline. The wait is not
    // shortened by having been relaunched, and nothing is re-saved: the
    // deadline is already durable.
    emit_entry_suspend_at(body, DIRECT_WAIT_DEADLINE_MS_LOCAL);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    // The wait is over. Poll before falling through — the same reasoning as the
    // blocking arm: this replay reached the delay through a checkpoint HIT, so
    // there is no `emit_checkpoint_save` here to fold signal handling into, and
    // without an explicit poll a woken chain of delays would run its remaining
    // steps having never looked for the cancel that arrived while it was parked.
    emit_check_signals_and_suspend(body, indices);
    body.instruction(&Instruction::Else);
    // MISS: first reach of this delay.
    emit_park_fresh(body, indices, output_ptr_local, output_len_local);
    body.instruction(&Instruction::End);
}

/// Compute `now + duration`, checkpoint it as this delay's deadline, and exit
/// `suspended(at(deadline))`. Never returns to the caller's flow.
fn emit_park_fresh(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    output_ptr_local: u32,
    output_len_local: u32,
) {
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
    body.instruction(&Instruction::I32Const(DIRECT_DELAY_DEADLINE_STATE_LEN));
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
}

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

        // Whether this artifact CAN park is a capability question, not a policy
        // one: only the invoke export has a success arm (`outcome::suspended`)
        // able to carry a wake. `wasi:cli/run` has none, and a workflow
        // published as an agent runs its durable steps inside the parent's
        // capability invoke — both must block.
        let can_park = indices.abi == crate::direct_wasm::component::WorkflowAbi::InvokeHostImports;
        if can_park {
            // WHETHER to park is decided at runtime, not here: `durationMs` may
            // be a reference (a template, an input field) that the emitter
            // cannot resolve, so the same artifact must be able to block on one
            // run and park on the next. Both arms are emitted and the guest
            // picks between them from the resolved duration.
            //
            // Both arms key off the SAME sleep key computed above, so the choice
            // is stable across a replay: the duration is recomputed from the
            // same replayed data and lands on the same side of the threshold.
            body.instruction(&Instruction::LocalGet(DIRECT_DELAY_DURATION_MS_LOCAL));
            body.instruction(&Instruction::I64Const(
                DIRECT_DURABLE_DELAY_PARK_THRESHOLD_MS,
            ));
            // Unsigned: the duration is a u64 carried in an i64 local.
            body.instruction(&Instruction::I64GeU);
            body.instruction(&Instruction::If(BlockType::Empty));
            emit_park_until_deadline(body, indices, output_ptr_local, output_len_local);
            body.instruction(&Instruction::Else);
            emit_blocking_durable_sleep(body, indices);
            body.instruction(&Instruction::End);
        } else {
            emit_blocking_durable_sleep(body, indices);
        }
    } else {
        body.instruction(&Instruction::LocalGet(DIRECT_DELAY_DURATION_MS_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.runtime_blocking_sleep));
        return_if_retptr_error(body, indices);
        // Same reasoning as the durable arm: a blocking sleep is a step that
        // spends real time without ever looking for a signal.
        emit_check_signals_and_suspend(body, indices);
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
