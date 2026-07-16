// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Concurrent Split lowering (docs/wasip3-parallelism.md Phase 3).
//!
//! When a Split's `parallelism` window is > 1 and its body is an ELIGIBLE
//! single-Agent subgraph, the item loop is emitted as chunked windows of a
//! memoized two-pass pipeline:
//!
//!   launch(chunk):  per item, best-effort input preparation + an
//!                   ASYNC-LOWERED `invoke` whose result lands in a per-item
//!                   SLOT; the subtask joins one waitable-set. Any preparation
//!                   failure just skips the launch (state stays EMPTY).
//!   drain(chunk):   `waitable-set.wait` until every launched subtask has
//!                   RETURNED (results are already written through the slot
//!                   retptrs), dropping each subtask handle.
//!   assemble(chunk):the EXACT sequential per-item pipeline, in input order —
//!                   same mapping/validation/debug-events/error routing — with
//!                   the agent invoke MEMOIZED: a filled slot is copied to the
//!                   canonical retptr scratch instead of re-invoking; an empty
//!                   slot falls back to the synchronous invoke.
//!
//! Correctness is therefore sequential-identical by construction (assemble IS
//! the sequential pipeline); the launch/drain passes are a pure overlap
//! optimization. Blocking in `waitable-set.wait` is legal because the invoke
//! export's task is async-TYPED (ABI v2); proven end-to-end by
//! `spikes/wasip3-stackful` (`run-both-sync`).
//!
//! V1 eligibility (anything else degrades to the sequential lowering):
//!   - Split: not durable, no retries, no timeout, any `dontStopOnFailed`.
//!   - Body: exactly one Agent step (terminal next), no retries, not durable,
//!     no breakpoint, and not a workflow-agent child (those share the parent's
//!     runtime host and checkpoint scope — concurrent invocations are a
//!     Phase-4 question).

use std::collections::BTreeMap;

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    load_retptr_list, load_retptr_tag, push_retptr_arg, push_segment_args, push_variables_args,
};
use super::agent::emit_agent_plan;
use super::split::{emit_loop_iteration_heap_reset, emit_value_store_retain};
use super::{
    DIRECT_PSPLIT_CHUNK_END_LOCAL, DIRECT_PSPLIT_CHUNK_START_LOCAL, DIRECT_PSPLIT_EVENT_OFFSET,
    DIRECT_PSPLIT_LAUNCH_LOCAL, DIRECT_PSPLIT_PENDING_LOCAL, DIRECT_PSPLIT_SLOT_RESULT_OFFSET,
    DIRECT_PSPLIT_SLOT_STRIDE, DIRECT_PSPLIT_SLOTS_LOCAL, DIRECT_PSPLIT_WS_LOCAL,
    DIRECT_SPLIT_COUNT_LOCAL, DIRECT_SPLIT_HEAP_BASE_LOCAL, DIRECT_SPLIT_INDEX_LOCAL,
    DIRECT_SPLIT_ITEM_LEN_LOCAL, DIRECT_SPLIT_ITEM_PTR_LOCAL, DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL,
    DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL, DIRECT_SPLIT_RESULTS_LEN_LOCAL,
    DIRECT_SPLIT_RESULTS_PTR_LOCAL, DIRECT_SPLIT_VARIABLES_LEN_LOCAL,
    DIRECT_SPLIT_VARIABLES_PTR_LOCAL, DirectCoreFunctionIndices, DirectCoreStaticData,
    DirectRunPlan, DirectVariables,
};

/// Subtask state code: the call fully resolved (result written through the
/// slot retptr). Low 4 bits of an async-lowered call's i32 status, and the
/// `state` payload of a SUBTASK completion event.
const SUBTASK_RETURNED: i32 = 2;

/// Ceiling on how many instances of ONE agent a parallel Split composes. A
/// sync-lifted instance serializes concurrent entries (component-instance
/// lock), so K-way overlap of one agent's calls needs K instances; each costs
/// its own linear memory, hence the cap.
pub(in crate::direct_wasm) const PARALLEL_POOL_MAX: u32 = 4;

/// Component id of pool member `n` for `agent_component_id`. Member 0 is the
/// real agent; members 1.. are extra instantiations of the SAME package wired
/// to phantom import names (`runtara:agent-<id>-par<n>/capabilities`).
pub(in crate::direct_wasm) fn pool_member_component_id(base: &str, member: u32) -> String {
    if member == 0 {
        base.to_string()
    } else {
        format!("{base}-par{member}")
    }
}

/// Effective pool size for a requested window.
pub(in crate::direct_wasm) fn pool_size_for_window(window: u32) -> u32 {
    window.clamp(1, PARALLEL_POOL_MAX)
}

/// Borrowed fields of the eligible single-Agent Split body.
pub(super) struct ParallelAgentBody<'a> {
    pub(super) step_id: &'a str,
    pub(super) agent_id: u32,
    pub(super) agent_component_id: &'a str,
    pub(super) input_mapping_id: u32,
    pub(super) durable_checkpoint: bool,
    pub(super) max_retries: u32,
    pub(super) retry_delay_ms: u64,
    pub(super) rate_limit_budget_ms: u64,
    pub(super) next_plan: &'a DirectRunPlan,
    pub(super) error_plan: Option<&'a super::DirectErrorRoutePlan>,
}

/// Eligibility for THIS split node: `Some(body)` when the requested window may
/// run concurrently. `static_data` is consulted to exclude workflow-agent
/// children.
pub(super) fn parallel_agent_body<'a>(
    static_data: &DirectCoreStaticData,
    parallel_window: Option<u32>,
    durable: bool,
    max_retries: u32,
    timeout_ms: Option<u64>,
    nested_plan: &'a DirectRunPlan,
) -> Option<ParallelAgentBody<'a>> {
    if !static_data.parallel_enabled {
        return None;
    }
    let window = parallel_window?;
    // Split-level durability is fine: the whole-split checkpoint if/else wraps
    // the item region (parallel windows included) unchanged. Split RETRIES and
    // TIMEOUT add extra frame blocks around the item region whose branch-depth
    // interplay is not wired for the parallel arm yet — sequential fallback.
    let _ = durable;
    if window <= 1 || max_retries > 0 || timeout_ms.is_some() {
        return None;
    }
    let DirectRunPlan::Agent {
        step_id,
        agent_id,
        agent_component_id,
        input_mapping_id,
        durable_checkpoint,
        breakpoint,
        max_retries: agent_retries,
        retry_delay_ms,
        rate_limit_budget_ms,
        next_plan,
        error_plan,
        ..
    } = nested_plan
    else {
        return None;
    };
    // Agent RETRIES are fine: attempt 1 consumes the memoized launch result
    // (consume-once slots), attempts 2+ re-invoke synchronously with the
    // standard backoff. Agent DURABILITY is fine too: the launch pass gates on
    // the step-level checkpoint (a HIT never launches, so replay cannot
    // double side effects), and assemble re-runs the standard durable block.
    if *breakpoint || static_data.agent_is_workflow_agent(*agent_id) {
        return None;
    }
    // Any continuation after the Agent is fine: the launch pass only fronts
    // the agent invoke itself; assemble runs the FULL body (agent + its
    // next_plan chain) sequentially with the invoke memoized.
    Some(ParallelAgentBody {
        step_id,
        agent_id: *agent_id,
        agent_component_id,
        input_mapping_id: *input_mapping_id,
        durable_checkpoint: *durable_checkpoint,
        max_retries: *agent_retries,
        retry_delay_ms: *retry_delay_ms,
        rate_limit_budget_ms: *rate_limit_budget_ms,
        next_plan,
        error_plan: error_plan.as_ref(),
    })
}

fn collect_error_route(
    static_data: &DirectCoreStaticData,
    error_plan: &super::DirectErrorRoutePlan,
    out: &mut BTreeMap<String, u32>,
) {
    for branch in &error_plan.branches {
        collect_parallel_agent_components(static_data, &branch.plan, out);
    }
    if let Some(default_plan) = &error_plan.default_plan {
        collect_parallel_agent_components(static_data, default_plan, out);
    }
}

/// Every agent referenced by an eligible parallel Split anywhere in the plan,
/// mapped to its POOL SIZE (the max across splits). Pool members each get an
/// `[async-lower]invoke` core import; members 1.. additionally get phantom
/// world imports + extra wac instantiations.
pub(in crate::direct_wasm) fn parallel_agent_pools(
    static_data: &DirectCoreStaticData,
    plan: &DirectRunPlan,
) -> BTreeMap<String, u32> {
    let mut pools = BTreeMap::new();
    collect_parallel_agent_components(static_data, plan, &mut pools);
    pools
}

fn collect_parallel_agent_components(
    static_data: &DirectCoreStaticData,
    plan: &DirectRunPlan,
    out: &mut BTreeMap<String, u32>,
) {
    use DirectRunPlan as P;
    match plan {
        P::Split {
            parallel_window,
            durable,
            max_retries,
            timeout_ms,
            nested_plan,
            next_plan,
            error_plan,
            ..
        } => {
            if let Some(body) = parallel_agent_body(
                static_data,
                *parallel_window,
                *durable,
                *max_retries,
                *timeout_ms,
                nested_plan,
            ) {
                let pool =
                    pool_size_for_window(parallel_window.expect("eligible body implies a window"));
                let entry = out.entry(body.agent_component_id.to_string()).or_insert(1);
                *entry = (*entry).max(pool);
            }
            collect_parallel_agent_components(static_data, nested_plan, out);
            collect_parallel_agent_components(static_data, next_plan, out);
            if let Some(error_plan) = error_plan {
                collect_error_route(static_data, error_plan, out);
            }
        }
        P::Filter { next_plan, .. }
        | P::SwitchValue { next_plan, .. }
        | P::GroupBy { next_plan, .. }
        | P::Delay { next_plan, .. }
        | P::Log { next_plan, .. } => {
            collect_parallel_agent_components(static_data, next_plan, out);
        }
        P::SwitchRoute {
            branches,
            default_plan,
            merge_plan,
            ..
        } => {
            for branch in branches {
                collect_parallel_agent_components(static_data, &branch.plan, out);
            }
            collect_parallel_agent_components(static_data, default_plan, out);
            if let Some(merge_plan) = merge_plan {
                collect_parallel_agent_components(static_data, merge_plan, out);
            }
        }
        P::EdgeRoute {
            branches,
            default_plan,
            merge_plan,
        } => {
            for branch in branches {
                collect_parallel_agent_components(static_data, &branch.plan, out);
            }
            collect_parallel_agent_components(static_data, default_plan, out);
            if let Some(merge_plan) = merge_plan {
                collect_parallel_agent_components(static_data, merge_plan, out);
            }
        }
        P::While {
            nested_plan,
            next_plan,
            error_plan,
            ..
        } => {
            collect_parallel_agent_components(static_data, nested_plan, out);
            collect_parallel_agent_components(static_data, next_plan, out);
            if let Some(error_plan) = error_plan {
                collect_error_route(static_data, error_plan, out);
            }
        }
        P::EmbedWorkflow {
            child_plan,
            next_plan,
            error_plan,
            ..
        } => {
            collect_parallel_agent_components(static_data, child_plan, out);
            collect_parallel_agent_components(static_data, next_plan, out);
            if let Some(error_plan) = error_plan {
                collect_error_route(static_data, error_plan, out);
            }
        }
        P::Agent {
            next_plan,
            error_plan,
            ..
        }
        | P::AiAgent {
            next_plan,
            error_plan,
            ..
        }
        | P::AiAgentLoop {
            next_plan,
            error_plan,
            ..
        } => {
            collect_parallel_agent_components(static_data, next_plan, out);
            if let Some(error_plan) = error_plan {
                collect_error_route(static_data, error_plan, out);
            }
        }
        P::WaitForSignal {
            on_wait_plan,
            next_plan,
            error_plan,
            ..
        } => {
            if let Some(on_wait_plan) = on_wait_plan {
                collect_parallel_agent_components(static_data, on_wait_plan, out);
            }
            collect_parallel_agent_components(static_data, next_plan, out);
            if let Some(error_plan) = error_plan {
                collect_error_route(static_data, error_plan, out);
            }
        }
        P::Conditional {
            true_plan,
            false_plan,
            merge_plan,
            ..
        } => {
            collect_parallel_agent_components(static_data, true_plan, out);
            collect_parallel_agent_components(static_data, false_plan, out);
            if let Some(merge_plan) = merge_plan {
                collect_parallel_agent_components(static_data, merge_plan, out);
            }
        }
        P::Error { .. } | P::Finish { .. } | P::Join | P::ImplicitFinish => {}
    }
}

fn mem32() -> wasm_encoder::MemArg {
    wasm_encoder::MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }
}

/// The chunked launch/drain/assemble item pipeline. Emitted INSIDE the split
/// prologue (frames, source, count, results and heap watermark already set
/// up by `emit_split_plan`), replacing the sequential item loop. The caller
/// guarantees: no retries, not durable, no timeout — so no retry frame, no
/// checkpoint block, no deadline checks exist around this.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_parallel_split_items(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    split_id: u32,
    window: u32,
    dont_stop_on_failed: bool,
    has_error_plan: bool,
    parallel: &ParallelAgentBody<'_>,
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
    workflow_log_kind: &super::DirectDataSegment,
    workflow_error_kind: &super::DirectDataSegment,
    active_iteration_failure_target: Option<super::DirectFailureTarget>,
    outer_iteration_failure_target: Option<super::DirectFailureTarget>,
    split_iteration_failure_target: super::DirectFailureTarget,
    fresh_failure_target: Option<super::DirectFailureTarget>,
    variables: DirectVariables<'_>,
) {
    let pool_size = pool_size_for_window(window);
    let invoke_pool: Vec<&super::DirectAgentInvokeImport> = (0..pool_size)
        .map(|member| {
            let component_id = pool_member_component_id(parallel.agent_component_id, member);
            indices
                .agent_invokes_async
                .get(&component_id)
                .expect("parallel split bodies have matching async pool imports")
        })
        .collect();
    let capability_id = static_data
        .agent_capability_id(parallel.agent_id)
        .expect("parallel split bodies have static capability ids");
    let ws_new = indices
        .waitable_set_new
        .expect("parallel split compiles import the waitable builtins");
    let ws_wait = indices
        .waitable_set_wait
        .expect("parallel split compiles import the waitable builtins");
    let ws_drop = indices
        .waitable_set_drop
        .expect("parallel split compiles import the waitable builtins");
    let waitable_join = indices
        .waitable_join
        .expect("parallel split compiles import the waitable builtins");
    let subtask_drop = indices
        .subtask_drop
        .expect("parallel split compiles import the waitable builtins");

    // item cursor starts at 0 (set by the caller, mirroring sequential).
    body.instruction(&Instruction::Block(BlockType::Empty)); // $chunks_done
    body.instruction(&Instruction::Loop(BlockType::Empty)); // $chunks
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_COUNT_LOCAL));
    body.instruction(&Instruction::I32GeU);
    body.instruction(&Instruction::BrIf(1));

    // Chunk-level heap reset (the sequential path does this per ITEM; slots
    // and per-item buffers must survive to assemble, so a chunk is the unit).
    emit_loop_iteration_heap_reset(
        body,
        DIRECT_SPLIT_HEAP_BASE_LOCAL,
        DIRECT_SPLIT_RESULTS_PTR_LOCAL,
        DIRECT_SPLIT_RESULTS_LEN_LOCAL,
    );
    emit_value_store_retain(
        body,
        indices,
        DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL,
        DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL,
        DIRECT_SPLIT_RESULTS_PTR_LOCAL,
        DIRECT_SPLIT_RESULTS_LEN_LOCAL,
    );

    // chunk_start = INDEX; chunk_end = min(INDEX + window, COUNT).
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_CHUNK_START_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    // window == u32::MAX means "unlimited": saturate instead of wrapping.
    if window == u32::MAX {
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_COUNT_LOCAL));
        body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_CHUNK_END_LOCAL));
        body.instruction(&Instruction::Drop);
    } else {
        body.instruction(&Instruction::I32Const(window as i32));
        body.instruction(&Instruction::I32Add);
        body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_CHUNK_END_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_CHUNK_END_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_COUNT_LOCAL));
        body.instruction(&Instruction::I32GtU);
        body.instruction(&Instruction::If(BlockType::Empty));
        body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_COUNT_LOCAL));
        body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_CHUNK_END_LOCAL));
        body.instruction(&Instruction::End);
    }

    // slots = bump(align8(global0), chunk_len * STRIDE), zero-filled. The
    // bump pointer is byte-granular (string allocations), but the slot
    // retptrs receive canonical-ABI stores that require natural alignment.
    body.instruction(&Instruction::GlobalGet(0));
    body.instruction(&Instruction::I32Const(7));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::I32Const(-8));
    body.instruction(&Instruction::I32And);
    body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_SLOTS_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_CHUNK_END_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_CHUNK_START_LOCAL));
    body.instruction(&Instruction::I32Sub);
    body.instruction(&Instruction::I32Const(DIRECT_PSPLIT_SLOT_STRIDE));
    body.instruction(&Instruction::I32Mul);
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_SLOTS_LOCAL));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::GlobalSet(0));
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_SLOTS_LOCAL));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::GlobalGet(0));
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_SLOTS_LOCAL));
    body.instruction(&Instruction::I32Sub);
    body.instruction(&Instruction::MemoryFill(0));

    // ws = waitable-set.new(); pending = 0.
    body.instruction(&Instruction::Call(ws_new));
    body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_WS_LOCAL));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_PENDING_LOCAL));

    // ── LAUNCH pass ─────────────────────────────────────────────────────────
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_CHUNK_START_LOCAL));
    body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_LAUNCH_LOCAL));
    body.instruction(&Instruction::Block(BlockType::Empty)); // $launch_done
    body.instruction(&Instruction::Loop(BlockType::Empty)); // $launch
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_LAUNCH_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_CHUNK_END_LOCAL));
    body.instruction(&Instruction::I32GeU);
    body.instruction(&Instruction::BrIf(1));

    body.instruction(&Instruction::Block(BlockType::Empty)); // $skip
    // Any retptr error below (mapping, validation, connection) skips the
    // launch — the slot stays EMPTY and assemble reproduces the exact failure
    // through the sequential pipeline.
    let skip_on_error = |body: &mut WasmFunction| {
        load_retptr_tag(body);
        body.instruction(&Instruction::BrIf(0));
    };

    // item = split-item(split_id, parent_source, i)
    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_LAUNCH_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_item));
    skip_on_error(body);
    load_retptr_list(
        body,
        DIRECT_SPLIT_ITEM_PTR_LOCAL,
        DIRECT_SPLIT_ITEM_LEN_LOCAL,
    );

    // split-validate-input(split_id, item, i)
    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_LAUNCH_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_validate_input));
    skip_on_error(body);

    // iteration variables
    body.instruction(&Instruction::I32Const(split_id as i32));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_PARENT_SOURCE_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_LEN_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_LAUNCH_LOCAL));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_split_iteration_variables));
    skip_on_error(body);
    load_retptr_list(
        body,
        DIRECT_SPLIT_VARIABLES_PTR_LOCAL,
        DIRECT_SPLIT_VARIABLES_LEN_LOCAL,
    );

    // per-item source = build-source(item, iteration variables, static steps)
    body.instruction(&Instruction::I32Const(static_data.steps.offset));
    body.instruction(&Instruction::LocalSet(steps_ptr_local));
    body.instruction(&Instruction::I32Const(static_data.steps.len_i32()));
    body.instruction(&Instruction::LocalSet(steps_len_local));
    let iteration_variables = DirectVariables::Locals {
        ptr_local: DIRECT_SPLIT_VARIABLES_PTR_LOCAL,
        len_local: DIRECT_SPLIT_VARIABLES_LEN_LOCAL,
    };
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_ITEM_LEN_LOCAL));
    push_variables_args(body, iteration_variables);
    body.instruction(&Instruction::LocalGet(steps_ptr_local));
    body.instruction(&Instruction::LocalGet(steps_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_build_source));
    skip_on_error(body);
    load_retptr_list(body, source_ptr_local, source_len_local);

    // Durable agents: skip the speculative launch when the step-level
    // checkpoint already has a result — assemble's durable block replays the
    // HIT without any invoke, and replay must not double side effects.
    if parallel.durable_checkpoint {
        body.instruction(&Instruction::I32Const(parallel.agent_id as i32));
        body.instruction(&Instruction::LocalGet(source_ptr_local));
        body.instruction(&Instruction::LocalGet(source_len_local));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_agent_cache_key));
        skip_on_error(body);
        load_retptr_list(body, route_ptr_local, route_len_local);
        body.instruction(&Instruction::LocalGet(route_ptr_local));
        body.instruction(&Instruction::LocalGet(route_len_local));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.runtime_get_checkpoint));
        skip_on_error(body);
        super::abi::emit_get_checkpoint_has_value(body);
        body.instruction(&Instruction::BrIf(0)); // HIT -> skip launch
    }

    // agent input = apply-mapping(mapping_id, source)
    body.instruction(&Instruction::I32Const(parallel.input_mapping_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_apply_mapping));
    skip_on_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);

    // agent-validate-input: a NON-EMPTY ok payload is a validation error —
    // skip (assemble reproduces it with full debug/error routing).
    body.instruction(&Instruction::I32Const(parallel.agent_id as i32));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_validate_input));
    skip_on_error(body);
    load_retptr_list(body, route_ptr_local, route_len_local);
    body.instruction(&Instruction::LocalGet(route_len_local));
    body.instruction(&Instruction::BrIf(0)); // -> $skip

    // connection injection (in-band `_connection`); no-op when connectionless.
    if static_data.agent_has_connection(parallel.agent_id) {
        body.instruction(&Instruction::I32Const(parallel.agent_id as i32));
        body.instruction(&Instruction::LocalGet(output_ptr_local));
        body.instruction(&Instruction::LocalGet(output_len_local));
        body.instruction(&Instruction::LocalGet(source_ptr_local));
        body.instruction(&Instruction::LocalGet(source_len_local));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_agent_connection_input));
        skip_on_error(body);
        load_retptr_list(body, output_ptr_local, output_len_local);
    }

    // slot_ptr = slots + (i - chunk_start) * STRIDE
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_LAUNCH_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_CHUNK_START_LOCAL));
    body.instruction(&Instruction::I32Sub);
    body.instruction(&Instruction::I32Const(DIRECT_PSPLIT_SLOT_STRIDE));
    body.instruction(&Instruction::I32Mul);
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_SLOTS_LOCAL));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(route_ptr_local)); // scratch: slot ptr

    // status = [async-lower]invoke(cap-id, input, slot_retptr), round-robined
    // across the instance pool so concurrent calls don't serialize on one
    // callee instance's exclusivity lock.
    let emit_pool_call = |body: &mut WasmFunction, member: &super::DirectAgentInvokeImport| {
        push_segment_args(body, capability_id);
        body.instruction(&Instruction::LocalGet(output_ptr_local));
        body.instruction(&Instruction::LocalGet(output_len_local));
        body.instruction(&Instruction::LocalGet(route_ptr_local));
        body.instruction(&Instruction::I32Const(DIRECT_PSPLIT_SLOT_RESULT_OFFSET));
        body.instruction(&Instruction::I32Add);
        body.instruction(&Instruction::Call(member.function_index));
        body.instruction(&Instruction::LocalSet(route_len_local)); // scratch: status
    };
    if invoke_pool.len() == 1 {
        emit_pool_call(body, invoke_pool[0]);
    } else {
        // sel = (i - chunk_start) % pool_size, dispatched as an if/else chain
        // (pool sizes are tiny). Each arm re-pushes its own operands.
        body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_LAUNCH_LOCAL));
        body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_CHUNK_START_LOCAL));
        body.instruction(&Instruction::I32Sub);
        body.instruction(&Instruction::I32Const(invoke_pool.len() as i32));
        body.instruction(&Instruction::I32RemU);
        body.instruction(&Instruction::LocalSet(route_len_local)); // scratch: sel
        for (member_index, member) in invoke_pool.iter().enumerate() {
            let last = member_index == invoke_pool.len() - 1;
            if last {
                emit_pool_call(body, member);
            } else {
                body.instruction(&Instruction::LocalGet(route_len_local));
                body.instruction(&Instruction::I32Const(member_index as i32));
                body.instruction(&Instruction::I32Eq);
                body.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
                emit_pool_call(body, member);
                body.instruction(&Instruction::Else);
            }
        }
        for _ in 0..invoke_pool.len() - 1 {
            body.instruction(&Instruction::End);
        }
    }

    // slot.state = 1 (launched); on eager RETURNED it is still "filled" —
    // assemble only distinguishes 0 (empty) from non-zero (result present).
    body.instruction(&Instruction::LocalGet(route_ptr_local));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Store(mem32()));

    // if (status & 0xF) != RETURNED { waitable.join(status >> 4, ws); pending++ }
    body.instruction(&Instruction::LocalGet(route_len_local));
    body.instruction(&Instruction::I32Const(0xF));
    body.instruction(&Instruction::I32And);
    body.instruction(&Instruction::I32Const(SUBTASK_RETURNED));
    body.instruction(&Instruction::I32Ne);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(route_len_local));
    body.instruction(&Instruction::I32Const(4));
    body.instruction(&Instruction::I32ShrU);
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_WS_LOCAL));
    body.instruction(&Instruction::Call(waitable_join));
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_PENDING_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_PENDING_LOCAL));
    body.instruction(&Instruction::End);

    body.instruction(&Instruction::End); // $skip
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_LAUNCH_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_LAUNCH_LOCAL));
    body.instruction(&Instruction::Br(0)); // -> $launch
    body.instruction(&Instruction::End); // loop
    body.instruction(&Instruction::End); // $launch_done

    // ── DRAIN ───────────────────────────────────────────────────────────────
    // Wait until every launched subtask has RETURNED. Results are written
    // through the slot retptrs by the runtime before the completion event.
    body.instruction(&Instruction::Block(BlockType::Empty)); // $drained
    body.instruction(&Instruction::Loop(BlockType::Empty)); // $drain
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_PENDING_LOCAL));
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::BrIf(1));
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_WS_LOCAL));
    body.instruction(&Instruction::I32Const(DIRECT_PSPLIT_EVENT_OFFSET));
    body.instruction(&Instruction::Call(ws_wait));
    body.instruction(&Instruction::Drop); // event code; dispatch on the state payload
    body.instruction(&Instruction::I32Const(DIRECT_PSPLIT_EVENT_OFFSET + 4));
    body.instruction(&Instruction::I32Load(mem32()));
    body.instruction(&Instruction::I32Const(SUBTASK_RETURNED));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::I32Const(DIRECT_PSPLIT_EVENT_OFFSET));
    body.instruction(&Instruction::I32Load(mem32()));
    body.instruction(&Instruction::Call(subtask_drop));
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_PENDING_LOCAL));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Sub);
    body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_PENDING_LOCAL));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::Br(0)); // -> $drain
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End); // $drained
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_WS_LOCAL));
    body.instruction(&Instruction::Call(ws_drop));

    // ── ASSEMBLE pass ───────────────────────────────────────────────────────
    // The EXACT sequential per-item pipeline, in input order, with the invoke
    // memoized. Failure semantics (dontStopOnFailed buckets, onError routing,
    // fatal fail) are identical to the sequential lowering — every subtask has
    // already resolved, so error exits can never leak an in-flight call.
    body.instruction(&Instruction::Block(BlockType::Empty)); // $assemble_done
    body.instruction(&Instruction::Loop(BlockType::Empty)); // $assemble
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_CHUNK_END_LOCAL));
    body.instruction(&Instruction::I32GeU);
    body.instruction(&Instruction::BrIf(1));

    super::split::emit_split_item_pipeline(
        body,
        indices,
        static_data,
        track_events,
        variables,
        split_id,
        dont_stop_on_failed,
        has_error_plan,
        parallel,
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
        active_iteration_failure_target,
        outer_iteration_failure_target,
        split_iteration_failure_target,
        fresh_failure_target,
    );

    body.instruction(&Instruction::Br(0)); // -> $assemble
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End); // $assemble_done

    body.instruction(&Instruction::Br(0)); // -> $chunks
    body.instruction(&Instruction::End); // loop $chunks
    body.instruction(&Instruction::End); // $chunks_done
}

/// Assemble-phase agent body: compute the item's slot pointer into a scratch
/// local and run the standard Agent lowering with the memoized invoke.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_parallel_agent_body(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    parallel: &ParallelAgentBody<'_>,
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
    workflow_log_kind: &super::DirectDataSegment,
    workflow_error_kind: &super::DirectDataSegment,
    failure_target: Option<super::DirectFailureTarget>,
    handled_target: Option<super::DirectHandledTarget>,
) {
    // memo slot for the CURRENT item: slots + (index - chunk_start) * STRIDE,
    // stashed in the launch-cursor local (dead during assemble).
    body.instruction(&Instruction::LocalGet(DIRECT_SPLIT_INDEX_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_CHUNK_START_LOCAL));
    body.instruction(&Instruction::I32Sub);
    body.instruction(&Instruction::I32Const(DIRECT_PSPLIT_SLOT_STRIDE));
    body.instruction(&Instruction::I32Mul);
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_SLOTS_LOCAL));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_LAUNCH_LOCAL));

    emit_agent_plan(
        body,
        indices,
        static_data,
        track_events,
        variables,
        parallel.step_id,
        parallel.agent_id,
        parallel.agent_component_id,
        parallel.input_mapping_id,
        parallel.durable_checkpoint,
        false, // breakpoint (excluded by eligibility)
        parallel.max_retries,
        parallel.retry_delay_ms,
        parallel.rate_limit_budget_ms,
        parallel.next_plan,
        parallel.error_plan,
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
        Some(DIRECT_PSPLIT_LAUNCH_LOCAL),
    );
}
