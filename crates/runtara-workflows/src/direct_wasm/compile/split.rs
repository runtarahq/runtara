// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Split step lowering and dontStopOnFailed failure aggregation helpers.
//!
//! The most control-flow-dense step: fan out over a list, run the nested subgraph
//! per item, and aggregate the results — multiplexing retry x durable x
//! dontStopOnFailed x timeout x onError, each of which adds Wasm block nesting.
//! `emit_split_plan` is built around explicit operand-stack frames and
//! `DirectFailureTarget` depth math so branch targets stay correct under every
//! combination (including nested splits); a `dontStopOnFailed` item appends its
//! error and continues the loop, otherwise it fails fast. This file also hosts
//! `emit_split_append_error_payload_and_continue`, the central hook every other
//! step's failure path calls to feed an enclosing split's aggregation.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    emit_retptr_error_or_return, emit_retptr_error_target_or_return, load_retptr_list,
    load_retptr_tag, push_retptr_arg, push_retptr_i32_load, push_retptr_i64_load,
    return_if_retptr_error,
};
use super::agent_error::emit_agent_error_route_or_fail;
use super::checkpoint::{emit_checkpoint_lookup, emit_checkpoint_save};
use super::debug::{emit_step_breakpoint, emit_step_debug_event};
use super::dispatcher::emit_run_plan_mapping;
use super::embed_workflow::emit_embed_workflow_child_error_and_continue;
use super::mapping::emit_build_source;
use super::split_retry::{
    emit_split_advance_retry_attempt, emit_split_retry_before_attempt, emit_split_retry_condition,
    emit_split_retry_error_info,
};
use super::step_error::{
    emit_step_error_and_continue, pop_step_error_frame, push_step_error_frame,
};
use super::wait::emit_wait_on_wait_error_and_fail;
use super::{
    DIRECT_RET_U32_OK_OFFSET, DIRECT_RET_U64_OK_OFFSET, DIRECT_SPLIT_COUNT_LOCAL,
    DIRECT_SPLIT_DEADLINE_MS_LOCAL, DIRECT_SPLIT_FAILURE_COUNT_LOCAL,
    DIRECT_SPLIT_FAILURE_INDEX_LOCAL, DIRECT_SPLIT_FAILURE_ITEM_LEN_LOCAL,
    DIRECT_SPLIT_FAILURE_ITEM_PTR_LOCAL, DIRECT_SPLIT_FAILURE_PARENT_SOURCE_LEN_LOCAL,
    DIRECT_SPLIT_FAILURE_PARENT_SOURCE_PTR_LOCAL, DIRECT_SPLIT_FAILURE_RESULTS_LEN_LOCAL,
    DIRECT_SPLIT_FAILURE_RESULTS_PTR_LOCAL, DIRECT_SPLIT_FAILURE_VARIABLES_LEN_LOCAL,
    DIRECT_SPLIT_FAILURE_VARIABLES_PTR_LOCAL, DIRECT_SPLIT_INDEX_LOCAL,
    DIRECT_SPLIT_ITEM_LEN_LOCAL, DIRECT_SPLIT_ITEM_PTR_LOCAL, DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL,
    DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL, DIRECT_SPLIT_PARENT_STEPS_LEN_LOCAL,
    DIRECT_SPLIT_PARENT_STEPS_PTR_LOCAL, DIRECT_SPLIT_RATE_LIMIT_WAIT_TOTAL_LOCAL,
    DIRECT_SPLIT_RATE_LIMITED_LOCAL, DIRECT_SPLIT_RESULTS_LEN_LOCAL,
    DIRECT_SPLIT_RESULTS_PTR_LOCAL, DIRECT_SPLIT_RETRY_AFTER_TAG_LOCAL,
    DIRECT_SPLIT_RETRY_ATTEMPT_LOCAL, DIRECT_SPLIT_RETRY_ERROR_FLAG_LOCAL,
    DIRECT_SPLIT_RETRY_ERROR_LEN_LOCAL, DIRECT_SPLIT_RETRY_ERROR_PTR_LOCAL,
    DIRECT_SPLIT_RETRY_SLEEP_KEY_LEN_LOCAL, DIRECT_SPLIT_RETRY_SLEEP_KEY_PTR_LOCAL,
    DIRECT_SPLIT_RETRY_SLEEP_MS_LOCAL, DIRECT_SPLIT_RETRYABLE_LOCAL,
    DIRECT_SPLIT_VARIABLES_LEN_LOCAL, DIRECT_SPLIT_VARIABLES_PTR_LOCAL,
    DIRECT_STEP_ERROR_FLAG_LOCAL, DIRECT_STEP_ERROR_LEN_LOCAL, DIRECT_STEP_ERROR_PTR_LOCAL,
    DirectCoreFunctionIndices, DirectCoreStaticData, DirectDataSegment, DirectErrorRoutePlan,
    DirectFailureTarget, DirectHandledTarget, DirectRunPlan, DirectVariables,
    emit_runtime_fail_return,
};

fn push_split_frame(body: &mut WasmFunction) {
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_COUNT_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_VARIABLES_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_VARIABLES_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ERROR_FLAG_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRYABLE_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RATE_LIMITED_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_AFTER_TAG_LOCAL));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_RETRY_SLEEP_KEY_PTR_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_RETRY_SLEEP_KEY_LEN_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ERROR_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ERROR_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_SLEEP_MS_LOCAL));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_RATE_LIMIT_WAIT_TOTAL_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_DEADLINE_MS_LOCAL));
}

fn pop_split_frame(body: &mut WasmFunction) {
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_DEADLINE_MS_LOCAL));
    body.instruction(&Instruction::LocalSet(
        DIRECT_SPLIT_RATE_LIMIT_WAIT_TOTAL_LOCAL,
    ));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RETRY_SLEEP_MS_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RETRY_ERROR_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RETRY_ERROR_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(
        DIRECT_SPLIT_RETRY_SLEEP_KEY_LEN_LOCAL,
    ));
    body.instruction(&Instruction::LocalSet(
        DIRECT_SPLIT_RETRY_SLEEP_KEY_PTR_LOCAL,
    ));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RETRY_AFTER_TAG_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RATE_LIMITED_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RETRYABLE_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RETRY_ATTEMPT_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RETRY_ERROR_FLAG_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_VARIABLES_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_VARIABLES_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RESULTS_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RESULTS_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_ITEM_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_ITEM_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_COUNT_LOCAL));
}

fn push_split_failure_frame(body: &mut WasmFunction) {
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_FAILURE_COUNT_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_FAILURE_INDEX_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_FAILURE_ITEM_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_FAILURE_ITEM_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_FAILURE_RESULTS_PTR_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_FAILURE_RESULTS_LEN_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_FAILURE_PARENT_SOURCE_PTR_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_FAILURE_PARENT_SOURCE_LEN_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_FAILURE_VARIABLES_PTR_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_FAILURE_VARIABLES_LEN_LOCAL,
    ));
}

fn pop_split_failure_frame(body: &mut WasmFunction) {
    body.instruction(&Instruction::LocalSet(
        DIRECT_SPLIT_FAILURE_VARIABLES_LEN_LOCAL,
    ));
    body.instruction(&Instruction::LocalSet(
        DIRECT_SPLIT_FAILURE_VARIABLES_PTR_LOCAL,
    ));
    body.instruction(&Instruction::LocalSet(
        DIRECT_SPLIT_FAILURE_PARENT_SOURCE_LEN_LOCAL,
    ));
    body.instruction(&Instruction::LocalSet(
        DIRECT_SPLIT_FAILURE_PARENT_SOURCE_PTR_LOCAL,
    ));
    body.instruction(&Instruction::LocalSet(
        DIRECT_SPLIT_FAILURE_RESULTS_LEN_LOCAL,
    ));
    body.instruction(&Instruction::LocalSet(
        DIRECT_SPLIT_FAILURE_RESULTS_PTR_LOCAL,
    ));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_FAILURE_ITEM_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_FAILURE_ITEM_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_FAILURE_INDEX_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_FAILURE_COUNT_LOCAL));
}

fn sync_split_failure_frame(body: &mut WasmFunction) {
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_COUNT_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_FAILURE_COUNT_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_FAILURE_INDEX_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_FAILURE_ITEM_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_FAILURE_ITEM_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(
        DIRECT_SPLIT_FAILURE_RESULTS_PTR_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(
        DIRECT_SPLIT_FAILURE_RESULTS_LEN_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(
        DIRECT_SPLIT_FAILURE_PARENT_SOURCE_PTR_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(
        DIRECT_SPLIT_FAILURE_PARENT_SOURCE_LEN_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_VARIABLES_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(
        DIRECT_SPLIT_FAILURE_VARIABLES_PTR_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_VARIABLES_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(
        DIRECT_SPLIT_FAILURE_VARIABLES_LEN_LOCAL,
    ));
}

fn restore_split_frame_from_failure_frame(body: &mut WasmFunction) {
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_FAILURE_COUNT_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_COUNT_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_FAILURE_INDEX_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_FAILURE_ITEM_PTR_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_ITEM_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_FAILURE_ITEM_LEN_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_ITEM_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_FAILURE_RESULTS_PTR_LOCAL,
    ));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RESULTS_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_FAILURE_RESULTS_LEN_LOCAL,
    ));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RESULTS_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_FAILURE_PARENT_SOURCE_PTR_LOCAL,
    ));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_FAILURE_PARENT_SOURCE_LEN_LOCAL,
    ));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_FAILURE_VARIABLES_PTR_LOCAL,
    ));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_VARIABLES_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_FAILURE_VARIABLES_LEN_LOCAL,
    ));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_VARIABLES_LEN_LOCAL));
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_split_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    split_id: u32,
    durable: bool,
    breakpoint: bool,
    max_retries: u32,
    retry_delay_ms: u64,
    dont_stop_on_failed: bool,
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
    outer_failure_target: Option<DirectFailureTarget>,
    handled_target: Option<DirectHandledTarget>,
) {
    let has_error_plan = error_plan.is_some();
    // When the Split has an onError route, redirect every fatal failure (a
    // fail-fast item failure, a result/cache-key error, or retry exhaustion) to a
    // step-error capture block — the outermost block of this lowering — then route
    // the captured error to the handler after the loop, mirroring While onError.
    // dontStopOnFailed per-item aggregation is unaffected; only the fatal path
    // reaches the capture.
    let failure_target = if has_error_plan {
        Some(DirectFailureTarget::StepError { branch_depth: 0 })
    } else {
        outer_failure_target
    };

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

    if has_error_plan {
        body.instruction(&Instruction::LocalGet(steps_ptr_local));
        body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_PARENT_STEPS_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(steps_len_local));
        body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_PARENT_STEPS_LEN_LOCAL));
        body.instruction(&Instruction::I32Const(0));
        body.instruction(&Instruction::LocalSet(DIRECT_STEP_ERROR_FLAG_LOCAL));
    }

    if dont_stop_on_failed {
        push_split_failure_frame(body);
    }

    push_split_frame(body);
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(source_len_local));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));

    // Capture block: any fatal split failure (routed via failure_target =
    // StepError) branches to the end of this block, where the onError handler runs.
    if has_error_plan {
        body.instruction(&Instruction::Block(BlockType::Empty));
    }

    if durable {
        body.instruction(&Instruction::I32Const(split_id as i32));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_split_cache_key));
        emit_retptr_error_or_return(
            body,
            indices,
            failure_target,
            route_ptr_local,
            route_len_local,
        );
        load_retptr_list(body, route_ptr_local, route_len_local);

        emit_checkpoint_lookup(
            body,
            indices,
            route_ptr_local,
            route_len_local,
            output_ptr_local,
            output_len_local,
        );
        body.instruction(&Instruction::Else);
    }

    // Resolve the timeout deadline once, before the retry/item loop so it spans
    // all attempts. `timeout_ms` is a static config value; generated Rust parses
    // but does not enforce it, so direct mode is the first to honor the documented
    // "if exceeded, step fails" contract. The deadline lives in the Split frame so
    // nested splits cannot clobber it; a now-ms call error fails fast.
    if let Some(timeout_ms) = timeout_ms {
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.runtime_now_ms));
        emit_retptr_error_or_return(body, indices, None, output_ptr_local, output_len_local);
        push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
        body.instruction(&Instruction::I64Const(timeout_ms as i64));
        body.instruction(&Instruction::I64Add);
        body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_DEADLINE_MS_LOCAL));
    }

    let retry_enabled = max_retries > 0;
    let fresh_failure_target = if retry_enabled {
        Some(DirectFailureTarget::SplitRetry { branch_depth: 0 })
    } else {
        failure_target
    };
    if retry_enabled {
        body.instruction(&Instruction::I32Const(1));
        body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RETRY_ATTEMPT_LOCAL));
        body.instruction(&Instruction::I64Const(0));
        body.instruction(&Instruction::LocalSet(
            DIRECT_SPLIT_RATE_LIMIT_WAIT_TOTAL_LOCAL,
        ));
        body.instruction(&Instruction::Block(BlockType::Empty));
        body.instruction(&Instruction::Loop(BlockType::Empty));
        body.instruction(&Instruction::I32Const(0));
        body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RETRY_ERROR_FLAG_LOCAL));
        emit_split_retry_before_attempt(
            body,
            indices,
            static_data,
            durable,
            route_ptr_local,
            route_len_local,
            max_retries,
            retry_delay_ms,
        );
        body.instruction(&Instruction::Block(BlockType::Empty));
    }

    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_item_count));
    emit_retptr_error_or_return(
        body,
        indices,
        fresh_failure_target,
        route_ptr_local,
        route_len_local,
    );
    push_retptr_i32_load(body, DIRECT_RET_U32_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_COUNT_LOCAL));

    body.instruction(&Instruction::I32Const(split_id as i32));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_initial_results));
    emit_retptr_error_or_return(
        body,
        indices,
        fresh_failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(
        body,
        DIRECT_SPLIT_RESULTS_PTR_LOCAL,
        DIRECT_SPLIT_RESULTS_LEN_LOCAL,
    );

    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::Block(BlockType::Empty));
    body.instruction(&Instruction::Loop(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_COUNT_LOCAL));
    body.instruction(&Instruction::I32GeU);
    body.instruction(&Instruction::BrIf(1));

    // Enforce the wall-clock timeout before each item. A Split that exceeds its
    // deadline is a hard failure (not aggregated or retried): it fails the
    // workflow with the static SPLIT_TIMEOUT payload via runtime.fail, which is
    // depth-independent and therefore correct under retry, durable, and
    // dontStopOnFailed nesting alike.
    if timeout_ms.is_some() {
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.runtime_now_ms));
        emit_retptr_error_or_return(body, indices, None, output_ptr_local, output_len_local);
        push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_DEADLINE_MS_LOCAL));
        body.instruction(&Instruction::I64GeU);
        body.instruction(&Instruction::If(BlockType::Empty));
        body.instruction(&Instruction::I32Const(
            static_data.split_timeout_error.offset,
        ));
        body.instruction(&Instruction::LocalSet(output_ptr_local));
        body.instruction(&Instruction::I32Const(
            static_data.split_timeout_error.len_i32(),
        ));
        body.instruction(&Instruction::LocalSet(output_len_local));
        emit_runtime_fail_return(body, indices, output_ptr_local, output_len_local);
        body.instruction(&Instruction::End);
    }

    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_item));
    let outer_iteration_failure_target = fresh_failure_target.map(|target| target.nested(2));
    let split_iteration_failure_target = DirectFailureTarget::Split {
        split_id,
        branch_depth: 0,
    };
    let active_iteration_failure_target = if dont_stop_on_failed {
        Some(split_iteration_failure_target)
    } else {
        outer_iteration_failure_target
    };
    emit_retptr_error_or_return(
        body,
        indices,
        active_iteration_failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(
        body,
        DIRECT_SPLIT_ITEM_PTR_LOCAL,
        DIRECT_SPLIT_ITEM_LEN_LOCAL,
    );

    if dont_stop_on_failed {
        sync_split_failure_frame(body);
    }

    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_validate_input));
    if dont_stop_on_failed {
        emit_split_append_retptr_error_and_continue(
            body,
            indices,
            split_iteration_failure_target,
            route_ptr_local,
            route_len_local,
        );
    } else {
        emit_retptr_error_or_return(
            body,
            indices,
            outer_iteration_failure_target,
            route_ptr_local,
            route_len_local,
        );
    }

    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_iteration_variables));
    emit_retptr_error_or_return(
        body,
        indices,
        active_iteration_failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(
        body,
        DIRECT_SPLIT_VARIABLES_PTR_LOCAL,
        DIRECT_SPLIT_VARIABLES_LEN_LOCAL,
    );

    body.instruction(&Instruction::I32Const(static_data.steps.offset));
    body.instruction(&Instruction::LocalSet(steps_ptr_local));
    body.instruction(&Instruction::I32Const(static_data.steps.len_i32()));
    body.instruction(&Instruction::LocalSet(steps_len_local));

    let iteration_variables = DirectVariables::Locals {
        ptr_local: DIRECT_SPLIT_VARIABLES_PTR_LOCAL,
        len_local: DIRECT_SPLIT_VARIABLES_LEN_LOCAL,
    };
    emit_build_source(
        body,
        indices,
        iteration_variables,
        DIRECT_SPLIT_ITEM_PTR_LOCAL,
        DIRECT_SPLIT_ITEM_LEN_LOCAL,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        active_iteration_failure_target,
    );

    push_split_frame(body);
    // Save the step-error frame around the item body so a nested onError capture
    // (which shares the step-error locals) cannot leak its handled flag into this
    // split's post-loop error check. A fatal body failure branches out to the
    // capture before the restore, leaving the flag set for this split.
    if has_error_plan {
        push_step_error_frame(body);
    }
    body.instruction(&Instruction::Block(BlockType::Empty));
    emit_run_plan_mapping(
        body,
        indices,
        static_data,
        track_events,
        iteration_variables,
        nested_plan,
        DIRECT_SPLIT_ITEM_PTR_LOCAL,
        DIRECT_SPLIT_ITEM_LEN_LOCAL,
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
        active_iteration_failure_target.map(|target| target.nested(1)),
        Some(DirectHandledTarget { branch_depth: 0 }),
    );
    body.instruction(&Instruction::End);
    if has_error_plan {
        pop_step_error_frame(body);
    }
    pop_split_frame(body);

    if dont_stop_on_failed {
        sync_split_failure_frame(body);
    }

    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_validate_output));
    if dont_stop_on_failed {
        emit_split_append_retptr_error_and_continue(
            body,
            indices,
            split_iteration_failure_target,
            route_ptr_local,
            route_len_local,
        );
    } else {
        emit_retptr_error_or_return(
            body,
            indices,
            outer_iteration_failure_target,
            route_ptr_local,
            route_len_local,
        );
    }

    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_append_output));
    emit_retptr_error_or_return(
        body,
        indices,
        fresh_failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(
        body,
        DIRECT_SPLIT_RESULTS_PTR_LOCAL,
        DIRECT_SPLIT_RESULTS_LEN_LOCAL,
    );

    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::Br(0));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);

    if durable {
        body.instruction(&Instruction::I32Const(split_id as i32));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_LEN_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_split_result));
        emit_retptr_error_or_return(
            body,
            indices,
            fresh_failure_target,
            route_ptr_local,
            route_len_local,
        );
        load_retptr_list(body, output_ptr_local, output_len_local);

        body.instruction(&Instruction::I32Const(split_id as i32));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_split_cache_key));
        emit_retptr_error_or_return(
            body,
            indices,
            fresh_failure_target,
            route_ptr_local,
            route_len_local,
        );
        load_retptr_list(body, route_ptr_local, route_len_local);

        emit_checkpoint_save(
            body,
            indices,
            route_ptr_local,
            route_len_local,
            output_ptr_local,
            output_len_local,
        );
        // Close the checkpoint-lookup `if`/`else` and build the step output from
        // the (cached or computed) result. Under retry this is deferred until
        // after the retry blocks (the inner-attempt block must stay open so
        // `retry-after-attempt` sits inside the retry loop); see the
        // `retry_enabled && durable` tail below.
        if !retry_enabled {
            body.instruction(&Instruction::End);
            emit_split_durable_output_from_result(
                body,
                indices,
                split_id,
                output_ptr_local,
                output_len_local,
                steps_ptr_local,
                steps_len_local,
            );
        }
    } else {
        body.instruction(&Instruction::I32Const(split_id as i32));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_LEN_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_split_output));
        emit_retptr_error_or_return(
            body,
            indices,
            fresh_failure_target,
            route_ptr_local,
            route_len_local,
        );
        load_retptr_list(body, steps_ptr_local, steps_len_local);
    }

    if retry_enabled {
        // Close the inner-attempt block, run retry-after-attempt INSIDE the retry
        // loop (so a retryable failure re-iterates the loop), then close the
        // loop and the retry-outer block.
        body.instruction(&Instruction::End);
        emit_split_retry_after_attempt(
            body,
            indices,
            max_retries,
            retry_delay_ms,
            failure_target,
            if durable { 3 } else { 2 },
        );
        body.instruction(&Instruction::End);
        body.instruction(&Instruction::End);
        if durable {
            // Close the deferred checkpoint-lookup `if`/`else` (opened before the
            // retry) and build the step output from the cached-or-computed
            // result once the retry settled.
            body.instruction(&Instruction::End);
            emit_split_durable_output_from_result(
                body,
                indices,
                split_id,
                output_ptr_local,
                output_len_local,
                steps_ptr_local,
                steps_len_local,
            );
        }
    }

    if has_error_plan {
        body.instruction(&Instruction::End);
    }

    pop_split_frame(body);
    if dont_stop_on_failed {
        pop_split_failure_frame(body);
    }

    if let Some(error_plan) = error_plan {
        // A fatal split failure was captured: restore the parent steps context and
        // route the captured error through the shared onError machinery. On a
        // normal split completion the flag is unset and this is skipped.
        body.instruction(&Instruction::LocalGet(DIRECT_STEP_ERROR_FLAG_LOCAL));
        body.instruction(&Instruction::If(BlockType::Empty));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_STEPS_PTR_LOCAL));
        body.instruction(&Instruction::LocalSet(steps_ptr_local));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_STEPS_LEN_LOCAL));
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
            outer_failure_target.map(|target| target.nested(1)),
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
        outer_failure_target,
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
        outer_failure_target,
        handled_target,
    );
}

pub(super) fn emit_split_retry_error_and_continue(
    body: &mut WasmFunction,
    target: DirectFailureTarget,
    error_ptr_local: u32,
    error_len_local: u32,
) {
    let DirectFailureTarget::SplitRetry { branch_depth } = target else {
        panic!("SplitRetry failure target expected");
    };

    body.instruction(&Instruction::LocalGet(error_ptr_local));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RETRY_ERROR_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(error_len_local));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RETRY_ERROR_LEN_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_RETRY_ERROR_FLAG_LOCAL));
    body.instruction(&Instruction::Br(branch_depth));
}

/// Build the durable Split's step output from the checkpointed-or-computed
/// result (`split-output-from-result`), after the checkpoint-lookup `if`/`else`
/// has closed so it covers both the cached and the freshly-computed branches.
fn emit_split_durable_output_from_result(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    split_id: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
) {
    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_output_from_result));
    return_if_retptr_error(body);
    load_retptr_list(body, steps_ptr_local, steps_len_local);
}

fn emit_split_retry_after_attempt(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    max_retries: u32,
    retry_delay_ms: u64,
    failure_target: Option<DirectFailureTarget>,
    failure_extra_depth: u32,
) {
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RETRY_ERROR_FLAG_LOCAL));
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_split_retry_error_info(body, indices);
    emit_split_retry_condition(body, max_retries, retry_delay_ms);
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_split_advance_retry_attempt(body);
    body.instruction(&Instruction::Br(2));
    body.instruction(&Instruction::End);
    if let Some(failure_target) = failure_target {
        emit_split_append_error_payload_and_continue(
            body,
            indices,
            failure_target.nested(failure_extra_depth),
            DIRECT_SPLIT_RETRY_ERROR_PTR_LOCAL,
            DIRECT_SPLIT_RETRY_ERROR_LEN_LOCAL,
        );
    } else {
        emit_runtime_fail_return(
            body,
            indices,
            DIRECT_SPLIT_RETRY_ERROR_PTR_LOCAL,
            DIRECT_SPLIT_RETRY_ERROR_LEN_LOCAL,
        );
    }
    body.instruction(&Instruction::Else);
    // No item failed this attempt: exit the retry by breaking past the retry
    // loop to the retry-outer block (the flag `if` is depth 0, the retry loop
    // depth 1, the retry-outer block depth 2).
    body.instruction(&Instruction::Br(2));
    body.instruction(&Instruction::End);
}

pub(super) fn emit_split_append_retptr_error_and_continue(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    target: DirectFailureTarget,
    error_ptr_local: u32,
    error_len_local: u32,
) {
    let DirectFailureTarget::Split { .. } = target else {
        emit_retptr_error_target_or_return(body, indices, target, error_ptr_local, error_len_local);
        return;
    };
    load_retptr_tag(body);
    body.instruction(&Instruction::If(BlockType::Empty));
    load_retptr_list(body, error_ptr_local, error_len_local);
    emit_split_append_error_payload_and_continue(
        body,
        indices,
        target.nested(1),
        error_ptr_local,
        error_len_local,
    );
    body.instruction(&Instruction::End);
}

pub(super) fn emit_split_append_error_payload_and_continue(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    target: DirectFailureTarget,
    error_ptr_local: u32,
    error_len_local: u32,
) {
    let DirectFailureTarget::Split {
        split_id,
        branch_depth,
    } = target
    else {
        match target {
            DirectFailureTarget::SplitRetry { .. } => {
                emit_split_retry_error_and_continue(body, target, error_ptr_local, error_len_local);
            }
            DirectFailureTarget::WaitOnWait { .. } => {
                emit_wait_on_wait_error_and_fail(
                    body,
                    indices,
                    target,
                    error_ptr_local,
                    error_len_local,
                );
            }
            DirectFailureTarget::EmbedWorkflow { .. } => {
                emit_embed_workflow_child_error_and_continue(
                    body,
                    target,
                    error_ptr_local,
                    error_len_local,
                );
            }
            DirectFailureTarget::StepError { .. } => {
                emit_step_error_and_continue(body, target, error_ptr_local, error_len_local);
            }
            DirectFailureTarget::Split { .. } => unreachable!(),
        }
        return;
    };
    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_FAILURE_RESULTS_PTR_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(
        DIRECT_SPLIT_FAILURE_RESULTS_LEN_LOCAL,
    ));
    body.instruction(&Instruction::LocalGet(error_ptr_local));
    body.instruction(&Instruction::LocalGet(error_len_local));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_FAILURE_INDEX_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_append_error));
    return_if_retptr_error(body);
    load_retptr_list(
        body,
        DIRECT_SPLIT_FAILURE_RESULTS_PTR_LOCAL,
        DIRECT_SPLIT_FAILURE_RESULTS_LEN_LOCAL,
    );
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_FAILURE_INDEX_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_FAILURE_INDEX_LOCAL));
    restore_split_frame_from_failure_frame(body);
    body.instruction(&Instruction::Br(branch_depth));
}
