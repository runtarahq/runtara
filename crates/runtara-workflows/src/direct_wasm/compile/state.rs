// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! SetState / GetState lowering for the direct workflow core emitter.
//!
//! A state step keeps its state one of two ways ([`DirectStateMode`]):
//!
//! - **Published** (the outer durable run): SetState resolves and checks its
//!   values (`stdlib.state-patch`) and merges them through the host
//!   (`runtara:workflow/state.set`), which applies each step key once, so a
//!   replay needs no checkpoint. GetState reads the host (`state.get`) and
//!   checkpoints the read under its key, so a replay gets back what the first
//!   execution read.
//! - **Local** (an embedded child, a published workflow-agent, a non-durable
//!   workflow): the stdlib keeps the invocation's state
//!   (`stdlib.state-local-set` / `state-local-get`). Where the run can replay,
//!   SetState checkpoints the record it produced and GetState what it read; a
//!   replay restores them (`stdlib.state-local-restore`).
//!
//! A value that does not match `stateSchema`, or a write the host refuses, is
//! the step error `STATE_<CODE>` (`stdlib.state-error`), routed to the step's
//! onError handler or the enclosing failure target, else failing the run.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    emit_retptr_error_or_return, emit_retptr_error_or_step_fail, load_retptr_list, load_retptr_tag,
    push_retptr_arg, push_segment_args, return_if_retptr_error,
};
use super::agent_error::emit_agent_error_route_or_fail;
use super::checkpoint::{emit_checkpoint_lookup, emit_checkpoint_save};
use super::debug::{emit_step_breakpoint, emit_step_debug_error, emit_step_debug_event};
use super::dispatcher::emit_run_plan_mapping;
use super::mapping::emit_build_source;
use super::{
    DIRECT_AGENT_RETRY_ATTEMPT_LOCAL, DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL,
    DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL, DIRECT_STEP_ERROR_LEN_LOCAL, DIRECT_STEP_ERROR_PTR_LOCAL,
    DirectCoreFunctionIndices, DirectCoreStaticData, DirectDataSegment, DirectErrorRoutePlan,
    DirectFailureTarget, DirectHandledTarget, DirectRunPlan, DirectVariables,
};
use crate::direct_wasm::manifest::DirectStateMode;

// Locals: a state step is a leaf like an Agent step, so it borrows the Agent
// scratch locals, which nested plans may clobber but which never outlive the
// step. The error pair is the shared step-error pair.
const KEY_PTR: u32 = DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL;
const KEY_LEN: u32 = DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL;
const FAILED: u32 = DIRECT_AGENT_RETRY_ATTEMPT_LOCAL;
const ERROR_PTR: u32 = DIRECT_STEP_ERROR_PTR_LOCAL;
const ERROR_LEN: u32 = DIRECT_STEP_ERROR_LEN_LOCAL;

/// The state step being lowered.
pub(super) struct StateStep<'a> {
    pub(super) step_id: &'a str,
    /// SetState (`true`) or GetState.
    pub(super) write: bool,
    pub(super) mode: DirectStateMode,
    pub(super) breakpoint: bool,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_state_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step: StateStep<'_>,
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
    let step_id = step.step_id;
    let step_id_segment = static_data
        .step_id(step_id)
        .expect("run plan step ids are present in static data");

    emit_step_breakpoint(
        body,
        indices,
        static_data,
        step.breakpoint,
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

    let (local, checkpoint) = match step.mode {
        DirectStateMode::Published => (false, true),
        DirectStateMode::Local { checkpoint } => (true, checkpoint),
    };

    // key = stdlib.state-key(step, source, local): the host's write identity
    // (published) or the checkpoint key.
    if checkpoint {
        push_segment_args(body, step_id_segment);
        body.instruction(&Instruction::LocalGet(source_ptr_local));
        body.instruction(&Instruction::LocalGet(source_len_local));
        body.instruction(&Instruction::I32Const(i32::from(local)));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_state_key));
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
    }

    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(FAILED));

    let call = |body: &mut WasmFunction, function: u32| {
        push_segment_args(body, step_id_segment);
        body.instruction(&Instruction::LocalGet(source_ptr_local));
        body.instruction(&Instruction::LocalGet(source_len_local));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(function));
    };

    match (step.write, local) {
        // Published SetState: patch, then the host's once-per-key merge. The
        // host log makes a replay safe without a checkpoint.
        (true, false) => {
            call(body, indices.stdlib_state_patch);
            fail_if_retptr_error(body);
            load_retptr_list(body, output_ptr_local, output_len_local);
            body.instruction(&Instruction::LocalGet(KEY_PTR));
            body.instruction(&Instruction::LocalGet(KEY_LEN));
            body.instruction(&Instruction::LocalGet(output_ptr_local));
            body.instruction(&Instruction::LocalGet(output_len_local));
            push_retptr_arg(body);
            body.instruction(&Instruction::Call(indices.run_state().set));
            fail_if_retptr_error(body);
            body.instruction(&Instruction::End);
            body.instruction(&Instruction::End);
        }
        // Published GetState: a checkpointed read of the host.
        (false, false) => {
            emit_checkpoint_lookup(
                body,
                indices,
                KEY_PTR,
                KEY_LEN,
                output_ptr_local,
                output_len_local,
            );
            body.instruction(&Instruction::Else);
            body.instruction(&Instruction::LocalGet(KEY_PTR));
            body.instruction(&Instruction::LocalGet(KEY_LEN));
            push_retptr_arg(body);
            body.instruction(&Instruction::Call(indices.run_state().get));
            fail_if_retptr_error(body);
            load_retptr_list(body, output_ptr_local, output_len_local);
            emit_checkpoint_save(
                body,
                indices,
                KEY_PTR,
                KEY_LEN,
                output_ptr_local,
                output_len_local,
            );
            body.instruction(&Instruction::End);
            body.instruction(&Instruction::End);
        }
        // Local SetState: merge in the stdlib; checkpoint the record where
        // the run can replay.
        (true, true) => {
            if checkpoint {
                emit_checkpoint_lookup(
                    body,
                    indices,
                    KEY_PTR,
                    KEY_LEN,
                    output_ptr_local,
                    output_len_local,
                );
                body.instruction(&Instruction::Else);
            }
            call(body, indices.stdlib_state_local_set);
            fail_if_retptr_error(body);
            load_retptr_list(body, output_ptr_local, output_len_local);
            if checkpoint {
                emit_checkpoint_save(
                    body,
                    indices,
                    KEY_PTR,
                    KEY_LEN,
                    output_ptr_local,
                    output_len_local,
                );
            }
            body.instruction(&Instruction::End);
            if checkpoint {
                body.instruction(&Instruction::End);
            }
        }
        // Local GetState: read the stdlib; checkpoint the read where the run
        // can replay.
        (false, true) => {
            if checkpoint {
                emit_checkpoint_lookup(
                    body,
                    indices,
                    KEY_PTR,
                    KEY_LEN,
                    output_ptr_local,
                    output_len_local,
                );
                body.instruction(&Instruction::Else);
            }
            call(body, indices.stdlib_state_local_get);
            return_if_retptr_error(body, indices);
            load_retptr_list(body, output_ptr_local, output_len_local);
            if checkpoint {
                emit_checkpoint_save(
                    body,
                    indices,
                    KEY_PTR,
                    KEY_LEN,
                    output_ptr_local,
                    output_len_local,
                );
                body.instruction(&Instruction::End);
            }
        }
    }

    // A refused value or write, as the step's structured error.
    body.instruction(&Instruction::LocalGet(FAILED));
    body.instruction(&Instruction::If(BlockType::Empty));
    push_segment_args(body, step_id_segment);
    body.instruction(&Instruction::LocalGet(ERROR_PTR));
    body.instruction(&Instruction::LocalGet(ERROR_LEN));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_state_error));
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

    // Local state: make the record or read the invocation's state (a no-op
    // on a fresh write, the restore on replay) and take the step's output.
    if local {
        push_segment_args(body, step_id_segment);
        body.instruction(&Instruction::LocalGet(output_ptr_local));
        body.instruction(&Instruction::LocalGet(output_len_local));
        body.instruction(&Instruction::LocalGet(source_ptr_local));
        body.instruction(&Instruction::LocalGet(source_len_local));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_state_local_restore));
        emit_retptr_error_or_return(
            body,
            indices,
            failure_target,
            output_ptr_local,
            output_len_local,
        );
        load_retptr_list(body, output_ptr_local, output_len_local);
    }

    // steps = stdlib.state-output(step, value, source)
    push_segment_args(body, step_id_segment);
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_state_output));
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
