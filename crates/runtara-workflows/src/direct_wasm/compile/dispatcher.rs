// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Direct run-plan dispatcher lowering.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction, MemArg};

use super::abi::{emit_retptr_error_or_return, push_retptr_arg};
use super::agent::emit_agent_plan;
use super::debug::{emit_step_breakpoint, emit_step_debug_event};
use super::delay::emit_delay_plan;
use super::edge_route::emit_edge_route_dispatch;
use super::embed_workflow::emit_embed_workflow_plan;
use super::error_step::emit_error_plan;
use super::log::emit_log_plan;
use super::mapping::emit_apply_mapping;
use super::split::emit_split_plan;
use super::step_context::emit_step_context_plan;
use super::switch_route::emit_switch_route_plan;
use super::wait::emit_wait_for_signal_plan;
use super::while_loop::emit_while_plan;
use super::{
    DIRECT_RUN_RETPTR_OFFSET, DirectCoreFunctionIndices, DirectCoreStaticData, DirectDataSegment,
    DirectFailureTarget, DirectRunPlan, DirectVariables,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_run_plan_mapping(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    run_plan: &DirectRunPlan,
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
    match run_plan {
        DirectRunPlan::Finish {
            step_id,
            mapping_id,
            breakpoint,
        } => {
            emit_step_debug_event(
                body,
                indices,
                static_data,
                track_events,
                true,
                step_id,
                source_ptr_local,
                source_len_local,
                route_ptr_local,
                route_len_local,
            );
            emit_apply_mapping(
                body,
                indices,
                *mapping_id,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
                failure_target,
            );
            emit_step_breakpoint(
                body,
                indices,
                static_data,
                *breakpoint,
                step_id,
                source_ptr_local,
                source_len_local,
                route_ptr_local,
                route_len_local,
                route_ptr_local,
                route_len_local,
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
                route_ptr_local,
                route_len_local,
            );
        }
        DirectRunPlan::Filter {
            step_id,
            filter_id,
            breakpoint,
            next_plan,
        } => {
            emit_step_context_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                indices.stdlib_filter,
                *filter_id,
                *breakpoint,
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
        DirectRunPlan::SwitchValue {
            step_id,
            switch_id,
            breakpoint,
            next_plan,
        } => {
            emit_step_context_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                indices.stdlib_value_switch,
                *switch_id,
                *breakpoint,
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
        DirectRunPlan::SwitchRoute {
            step_id,
            switch_id,
            breakpoint,
            branches,
            default_plan,
        } => {
            emit_switch_route_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                *switch_id,
                *breakpoint,
                branches,
                default_plan,
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
        DirectRunPlan::EdgeRoute {
            branches,
            default_plan,
        } => {
            emit_edge_route_dispatch(
                body,
                indices,
                static_data,
                track_events,
                variables,
                branches,
                default_plan,
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
        DirectRunPlan::GroupBy {
            step_id,
            group_id,
            breakpoint,
            next_plan,
        } => {
            emit_step_context_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                indices.stdlib_group_by,
                *group_id,
                *breakpoint,
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
        DirectRunPlan::Split {
            step_id,
            split_id,
            durable,
            breakpoint,
            dont_stop_on_failed,
            nested_plan,
            next_plan,
        } => {
            emit_split_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                *split_id,
                *durable,
                *breakpoint,
                *dont_stop_on_failed,
                nested_plan,
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
        DirectRunPlan::While {
            step_id,
            while_id,
            breakpoint,
            nested_plan,
            next_plan,
        } => {
            emit_while_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                *while_id,
                *breakpoint,
                nested_plan,
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
        DirectRunPlan::EmbedWorkflow {
            step_id,
            input_mapping_id,
            durable,
            breakpoint,
            max_retries,
            retry_delay_ms,
            child_plan,
            next_plan,
            error_plan,
        } => {
            emit_embed_workflow_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                *input_mapping_id,
                *durable,
                *breakpoint,
                *max_retries,
                *retry_delay_ms,
                child_plan,
                next_plan,
                error_plan.as_ref(),
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
        DirectRunPlan::Delay {
            step_id,
            delay_id,
            durable,
            breakpoint,
            next_plan,
        } => {
            emit_delay_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                *delay_id,
                *durable,
                *breakpoint,
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
        DirectRunPlan::WaitForSignal {
            step_id,
            breakpoint,
            on_wait_plan,
            next_plan,
        } => {
            emit_wait_for_signal_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                *breakpoint,
                on_wait_plan.as_deref(),
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
        DirectRunPlan::Log {
            step_id,
            log_id,
            breakpoint,
            next_plan,
        } => {
            emit_log_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                *log_id,
                *breakpoint,
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
        DirectRunPlan::Agent {
            step_id,
            agent_id,
            agent_component_id,
            input_mapping_id,
            durable_checkpoint,
            breakpoint,
            max_retries,
            retry_delay_ms,
            rate_limit_budget_ms,
            next_plan,
            error_plan,
        } => {
            emit_agent_plan(
                body,
                indices,
                static_data,
                track_events,
                variables,
                step_id,
                *agent_id,
                agent_component_id,
                *input_mapping_id,
                *durable_checkpoint,
                *breakpoint,
                *max_retries,
                *retry_delay_ms,
                *rate_limit_budget_ms,
                next_plan,
                error_plan.as_ref(),
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
        DirectRunPlan::Error {
            step_id,
            error_id,
            breakpoint,
        } => {
            emit_error_plan(
                body,
                indices,
                static_data,
                track_events,
                step_id,
                *error_id,
                *breakpoint,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
                workflow_error_kind,
                failure_target,
            );
        }
        DirectRunPlan::Conditional {
            step_id,
            condition_id,
            breakpoint,
            true_plan,
            false_plan,
        } => {
            emit_step_breakpoint(
                body,
                indices,
                static_data,
                *breakpoint,
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
            body.instruction(&Instruction::I32Const(*condition_id as i32));
            body.instruction(&Instruction::LocalGet(source_ptr_local));
            body.instruction(&Instruction::LocalGet(source_len_local));
            push_retptr_arg(body);
            body.instruction(&Instruction::Call(indices.stdlib_eval_condition));
            emit_retptr_error_or_return(
                body,
                indices,
                failure_target,
                route_ptr_local,
                route_len_local,
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

            body.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
            body.instruction(&Instruction::I32Load8U(MemArg {
                offset: 4,
                align: 0,
                memory_index: 0,
            }));
            body.instruction(&Instruction::If(BlockType::Empty));
            emit_run_plan_mapping(
                body,
                indices,
                static_data,
                track_events,
                variables,
                true_plan,
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
                failure_target.map(|target| target.nested(1)),
            );
            body.instruction(&Instruction::Else);
            emit_run_plan_mapping(
                body,
                indices,
                static_data,
                track_events,
                variables,
                false_plan,
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
                failure_target.map(|target| target.nested(1)),
            );
            body.instruction(&Instruction::End);
        }
    }
}
