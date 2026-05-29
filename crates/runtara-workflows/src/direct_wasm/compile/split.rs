// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Split step lowering and dontStopOnFailed failure aggregation helpers.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    emit_retptr_error_target_or_return, load_retptr_list, load_retptr_tag, push_retptr_arg,
    push_retptr_i32_load, return_if_retptr_error,
};
use super::checkpoint::{emit_checkpoint_lookup, emit_checkpoint_save};
use super::debug::emit_step_debug_event;
use super::dispatcher::emit_run_plan_mapping;
use super::embed_workflow::emit_embed_workflow_child_error_and_continue;
use super::mapping::emit_build_source;
use super::wait::emit_wait_on_wait_error_and_fail;
use super::{
    DIRECT_RET_U32_OK_OFFSET, DIRECT_SPLIT_COUNT_LOCAL, DIRECT_SPLIT_INDEX_LOCAL,
    DIRECT_SPLIT_ITEM_LEN_LOCAL, DIRECT_SPLIT_ITEM_PTR_LOCAL, DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL,
    DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL, DIRECT_SPLIT_RESULTS_LEN_LOCAL,
    DIRECT_SPLIT_RESULTS_PTR_LOCAL, DIRECT_SPLIT_VARIABLES_LEN_LOCAL,
    DIRECT_SPLIT_VARIABLES_PTR_LOCAL, DirectCoreFunctionIndices, DirectCoreStaticData,
    DirectDataSegment, DirectFailureTarget, DirectRunPlan, DirectVariables,
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
}

fn pop_split_frame(body: &mut WasmFunction) {
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
    dont_stop_on_failed: bool,
    nested_plan: &DirectRunPlan,
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
) {
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

    push_split_frame(body);
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(source_len_local));
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));

    if durable {
        body.instruction(&Instruction::I32Const(split_id as i32));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_split_cache_key));
        return_if_retptr_error(body);
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

    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_item_count));
    return_if_retptr_error(body);
    push_retptr_i32_load(body, DIRECT_RET_U32_OK_OFFSET);
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_COUNT_LOCAL));

    body.instruction(&Instruction::I32Const(split_id as i32));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_initial_results));
    return_if_retptr_error(body);
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

    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_item));
    return_if_retptr_error(body);
    load_retptr_list(
        body,
        DIRECT_SPLIT_ITEM_PTR_LOCAL,
        DIRECT_SPLIT_ITEM_LEN_LOCAL,
    );

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
            DirectFailureTarget::Split {
                split_id,
                branch_depth: 0,
            },
            route_ptr_local,
            route_len_local,
        );
    } else {
        return_if_retptr_error(body);
    }

    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_iteration_variables));
    return_if_retptr_error(body);
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
        if dont_stop_on_failed {
            Some(DirectFailureTarget::Split {
                split_id,
                branch_depth: 0,
            })
        } else {
            failure_target
        },
    );

    if !dont_stop_on_failed {
        push_split_frame(body);
    }
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
        if dont_stop_on_failed {
            Some(DirectFailureTarget::Split {
                split_id,
                branch_depth: 0,
            })
        } else {
            failure_target
        },
    );
    if !dont_stop_on_failed {
        pop_split_frame(body);
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
            DirectFailureTarget::Split {
                split_id,
                branch_depth: 0,
            },
            route_ptr_local,
            route_len_local,
        );
    } else {
        return_if_retptr_error(body);
    }

    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_append_output));
    return_if_retptr_error(body);
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
        return_if_retptr_error(body);
        load_retptr_list(body, output_ptr_local, output_len_local);

        body.instruction(&Instruction::I32Const(split_id as i32));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_split_cache_key));
        return_if_retptr_error(body);
        load_retptr_list(body, route_ptr_local, route_len_local);

        emit_checkpoint_save(
            body,
            indices,
            route_ptr_local,
            route_len_local,
            output_ptr_local,
            output_len_local,
        );
        body.instruction(&Instruction::End);

        body.instruction(&Instruction::I32Const(split_id as i32));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
        body.instruction(&Instruction::LocalGet(output_ptr_local));
        body.instruction(&Instruction::LocalGet(output_len_local));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_split_output_from_result));
        return_if_retptr_error(body);
        load_retptr_list(body, steps_ptr_local, steps_len_local);
    } else {
        body.instruction(&Instruction::I32Const(split_id as i32));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_LEN_LOCAL));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_split_output));
        return_if_retptr_error(body);
        load_retptr_list(body, steps_ptr_local, steps_len_local);
    }

    pop_split_frame(body);
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
    );
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
            DirectFailureTarget::Split { .. } => unreachable!(),
        }
        return;
    };
    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_RESULTS_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(error_ptr_local));
    body.instruction(&Instruction::LocalGet(error_len_local));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_append_error));
    return_if_retptr_error(body);
    load_retptr_list(
        body,
        DIRECT_SPLIT_RESULTS_PTR_LOCAL,
        DIRECT_SPLIT_RESULTS_LEN_LOCAL,
    );
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::Br(branch_depth));
}
