// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Direct run-plan dispatcher lowering.
//!
//! The recursive heart of the emitter. `emit_run_plan_mapping` walks the
//! `DirectRunPlan` tree and emits instructions for each node — delegating leaf/
//! linear steps to their dedicated lowerers (`emit_agent_plan`, `emit_split_plan`,
//! …) and emitting structured control flow inline for branching ones. A
//! `Conditional`/`SwitchRoute`/`EdgeRoute` becomes a Wasm `if/else`; because a
//! graph diamond re-converges but Wasm has only structured control flow, the
//! shared continuation (`merge_plan`) is emitted *once* after the `End` at the
//! parent block depth — both arms reach it as a `Join` no-op and fall through —
//! while `failure_target`/`handled_target` are bumped via `.nested(n)` so error
//! `Br`s still target the right enclosing block.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction, MemArg};

use super::abi::{emit_retptr_error_or_step_fail, push_retptr_arg};
use super::agent::emit_agent_plan;
use super::debug::{emit_step_breakpoint, emit_step_debug_event};
use super::delay::emit_delay_plan;
use super::edge_route::emit_edge_route_dispatch;
use super::embed_workflow::emit_embed_workflow_plan;
use super::error_step::emit_error_plan;
use super::log::emit_log_plan;
use super::mapping::emit_apply_mapping_step_error;
use super::split::emit_split_plan;
use super::step_context::emit_step_context_plan;
use super::switch_route::emit_switch_route_plan;
use super::wait::emit_wait_for_signal_plan;
use super::while_loop::emit_while_plan;
use super::{
    DIRECT_CONDITION_RESULT_LOCAL, DIRECT_RUN_RETPTR_OFFSET, DirectCoreFunctionIndices,
    DirectCoreStaticData, DirectDataSegment, DirectFailureTarget, DirectHandledTarget,
    DirectRunPlan, DirectVariables,
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
    handled_target: Option<DirectHandledTarget>,
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
            emit_apply_mapping_step_error(
                body,
                indices,
                static_data,
                track_events,
                *mapping_id,
                step_id,
                source_ptr_local,
                source_len_local,
                output_ptr_local,
                output_len_local,
                route_ptr_local,
                route_len_local,
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
                handled_target,
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
                handled_target,
            );
        }
        DirectRunPlan::SwitchRoute {
            step_id,
            switch_id,
            breakpoint,
            branches,
            default_plan,
            merge_plan,
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
                handled_target,
            );
            // Diamond: all routes (and default) reach the merge as a `Join` and
            // fall through the nested dispatch, so the shared continuation runs
            // once here at the original depth.
            if let Some(merge_plan) = merge_plan {
                emit_run_plan_mapping(
                    body,
                    indices,
                    static_data,
                    track_events,
                    variables,
                    merge_plan,
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
        }
        DirectRunPlan::EdgeRoute {
            branches,
            default_plan,
            merge_plan,
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
                handled_target,
            );
            // Diamond: all conditioned branches (and default) reach the merge as a
            // `Join` and fall through the nested dispatch, so the shared
            // continuation runs once here at the original depth.
            if let Some(merge_plan) = merge_plan {
                emit_run_plan_mapping(
                    body,
                    indices,
                    static_data,
                    track_events,
                    variables,
                    merge_plan,
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
                handled_target,
            );
        }
        DirectRunPlan::Split {
            step_id,
            split_id,
            durable,
            breakpoint,
            max_retries,
            retry_delay_ms,
            dont_stop_on_failed,
            parallel_window,
            nested_plan,
            next_plan,
            error_plan,
            timeout_ms,
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
                *max_retries,
                *retry_delay_ms,
                *dont_stop_on_failed,
                *parallel_window,
                nested_plan,
                next_plan,
                error_plan.as_ref(),
                *timeout_ms,
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
        DirectRunPlan::While {
            step_id,
            while_id,
            breakpoint,
            nested_plan,
            next_plan,
            error_plan,
            timeout_ms,
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
                error_plan.as_ref(),
                *timeout_ms,
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
                handled_target,
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
                handled_target,
            );
        }
        DirectRunPlan::WaitForSignal {
            step_id,
            breakpoint,
            on_wait_plan,
            next_plan,
            error_plan,
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
                handled_target,
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
                handled_target,
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
                handled_target,
                indices.stdlib_agent_output,
                None,
            );
        }
        DirectRunPlan::AiAgent {
            step_id,
            agent_id,
            agent_component_id,
            input_mapping_id,
            durable_checkpoint,
            breakpoint,
            max_retries,
            retry_delay_ms,
            next_plan,
            error_plan,
        } => {
            // Single-shot AiAgent reuses the Agent invoke/checkpoint/retry path
            // (it is an invoke of `ai_tools`/`chat-completion`); only the output
            // transform differs (`ai-agent-output` builds the
            // `{response, iterations, toolCalls}` envelope). Retries are opt-in
            // via config.maxRetries (plan default 0 — LLM calls re-bill); no
            // rate-limit budget (ai-tools is not catalog-rate-limited).
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
                0,
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
                handled_target,
                indices.stdlib_ai_agent_output,
                None,
            );
        }
        DirectRunPlan::AiAgentLoop {
            step_id,
            agent_id,
            agent_component_id,
            input_mapping_id,
            durable_checkpoint,
            breakpoint,
            max_iterations,
            tools,
            memory,
            next_plan,
            error_plan,
        } => {
            super::ai_agent_loop::emit_ai_agent_loop_plan(
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
                *max_iterations,
                tools,
                memory.as_ref(),
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
                handled_target,
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
            merge_plan,
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
            // Capture the evaluated condition BEFORE the debug-end event below.
            // step-debug-end / custom-event reuse the shared retptr scratch and
            // overwrite the bool at offset 4, so reading it *after* the event
            // returned a clobbered (always-non-zero) byte — the Conditional then
            // always took the `true` branch whenever track-events was on.
            body.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
            body.instruction(&Instruction::I32Load8U(MemArg {
                offset: 4,
                align: 0,
                memory_index: 0,
            }));
            body.instruction(&Instruction::LocalSet(DIRECT_CONDITION_RESULT_LOCAL));

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

            body.instruction(&Instruction::LocalGet(DIRECT_CONDITION_RESULT_LOCAL));
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
                handled_target.map(|target| target.nested(1)),
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
                handled_target.map(|target| target.nested(1)),
            );
            body.instruction(&Instruction::End);
            // Diamond: both branches reach the merge as a `Join` (no-op) and fall
            // through the `if/else`, so the shared continuation is emitted once
            // here at the original block depth (not nested in the branches).
            if let Some(merge_plan) = merge_plan {
                emit_run_plan_mapping(
                    body,
                    indices,
                    static_data,
                    track_events,
                    variables,
                    merge_plan,
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
        }
        DirectRunPlan::ParallelBranches {
            branches,
            merge_plan,
        } => {
            super::branch_parallel::emit_parallel_branches(
                body,
                indices,
                static_data,
                track_events,
                variables,
                branches,
                merge_plan,
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
        DirectRunPlan::Join => {
            // A branch that reached its enclosing branching step's merge point;
            // the merge is emitted by the parent as the shared continuation, so
            // this falls through and emits nothing.
        }
        DirectRunPlan::ImplicitFinish => {
            // No explicit Finish step: complete the workflow with a `null` output
            // (the generated compiler returns `Ok(Value::Null)` in this case).
            // `runtime.complete` runs on `output_ptr/len` after the plan.
            body.instruction(&Instruction::I32Const(static_data.output_null.offset));
            body.instruction(&Instruction::LocalSet(output_ptr_local));
            body.instruction(&Instruction::I32Const(static_data.output_null.len_i32()));
            body.instruction(&Instruction::LocalSet(output_len_local));
        }
    }
}
