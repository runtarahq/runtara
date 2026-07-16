// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Concurrent parallel-branch lowering (docs/wasip3-parallel-branches-plan.md,
//! Phase 4a).
//!
//! An unconditional single-Agent fan-out `A → {AgentB, AgentC, …} → M` runs the
//! branch agents CONCURRENTLY instead of linearising them:
//!
//!   launch:   per branch, best-effort input preparation + an ASYNC-LOWERED
//!             `invoke` whose result lands in a per-branch SLOT; the subtask joins
//!             one waitable-set. Any preparation failure just skips the launch
//!             (the slot stays EMPTY).
//!   drain:    `waitable-set.wait` until every launched subtask has RETURNED.
//!   assemble: the EXACT sequential per-branch Agent lowering, in declaration
//!             order, with the agent invoke MEMOIZED — a filled slot is copied to
//!             the retptr scratch instead of re-invoking; an empty slot falls back
//!             to the synchronous invoke. Each branch's output is appended to the
//!             `steps` context, so the shared `merge_plan` (emitted once, after)
//!             sees every branch's result.
//!
//! Correctness is sequential-identical by construction (assemble IS the sequential
//! lowering); launch/drain is a pure overlap optimization. This holds because
//! independent DAG branches never reference one another — a branch's mapping
//! resolves only against its predecessors, so re-applying it during assemble (with
//! the `steps` context now carrying earlier siblings) yields the same agent input,
//! and the invoke is memoized regardless. Blocking in `waitable-set.wait` is legal
//! because the invoke export's task is async-TYPED (ABI v2).
//!
//! A sync-ABI build (`!parallel_enabled`) or a workflow-agent branch (which shares
//! the parent runtime host and checkpoint scope) degrades to emitting the branches
//! sequentially — no window, no memo — which is the same instruction stream the
//! linearised path would have produced.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    emit_entry_suspend_return, emit_get_checkpoint_has_value, emit_retptr_error_or_return,
    load_retptr_list, load_retptr_tag, push_retptr_arg, push_retptr_u8_load, push_segment_args,
};
use std::collections::BTreeMap;

use super::agent::emit_agent_plan;
use super::dispatcher::emit_run_plan_mapping;
use super::split_parallel::{
    emit_drain_pending, emit_join_if_pending, pool_member_component_id, pool_size_for_window,
};
use super::{
    DIRECT_PSPLIT_LAUNCH_LOCAL, DIRECT_PSPLIT_PENDING_LOCAL, DIRECT_PSPLIT_SIGNAL_LOCAL,
    DIRECT_PSPLIT_SLOT_RESULT_OFFSET, DIRECT_PSPLIT_SLOT_STRIDE, DIRECT_PSPLIT_SLOTS_LOCAL,
    DIRECT_PSPLIT_WS_LOCAL, DIRECT_RET_BOOL_OK_OFFSET, DirectCoreFunctionIndices,
    DirectCoreStaticData, DirectDataSegment, DirectErrorRoutePlan, DirectFailureTarget,
    DirectHandledTarget, DirectRunPlan, DirectVariables,
};

/// Slot state: an agent result has been launched into this slot (non-zero, so the
/// memoized invoke in assemble copies it instead of re-invoking).
const SLOT_AGENT_READY: i32 = 1;

fn mem32() -> wasm_encoder::MemArg {
    wasm_encoder::MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }
}

/// Borrowed fields of a single-Agent branch (`Agent { next_plan: Join, .. }`).
struct BranchAgent<'a> {
    step_id: &'a str,
    agent_id: u32,
    agent_component_id: &'a str,
    input_mapping_id: u32,
    durable_checkpoint: bool,
    max_retries: u32,
    retry_delay_ms: u64,
    rate_limit_budget_ms: u64,
    next_plan: &'a DirectRunPlan,
    error_plan: Option<&'a DirectErrorRoutePlan>,
}

fn branch_agent(plan: &DirectRunPlan) -> BranchAgent<'_> {
    let DirectRunPlan::Agent {
        step_id,
        agent_id,
        agent_component_id,
        input_mapping_id,
        durable_checkpoint,
        max_retries,
        retry_delay_ms,
        rate_limit_budget_ms,
        next_plan,
        error_plan,
        ..
    } = plan
    else {
        // `try_parallel_branches` only builds `ParallelBranches` from single-Agent
        // branches; the dispatcher never reaches here with anything else.
        unreachable!("parallel branch is not a single Agent step");
    };
    BranchAgent {
        step_id,
        agent_id: *agent_id,
        agent_component_id,
        input_mapping_id: *input_mapping_id,
        durable_checkpoint: *durable_checkpoint,
        max_retries: *max_retries,
        retry_delay_ms: *retry_delay_ms,
        rate_limit_budget_ms: *rate_limit_budget_ms,
        next_plan,
        error_plan: error_plan.as_ref(),
    }
}

/// A branch may run concurrently unless it targets a workflow-agent (shared
/// runtime host / checkpoint scope — a Phase-4c question).
fn branch_concurrent_eligible(static_data: &DirectCoreStaticData, plan: &DirectRunPlan) -> bool {
    !static_data.agent_is_workflow_agent(branch_agent(plan).agent_id)
}

/// Emit one branch's Agent lowering (assemble pass, or the whole thing on the
/// sequential fallback). `memo_slot` copies the pre-launched result when set.
#[allow(clippy::too_many_arguments)]
fn emit_branch_agent(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    branch: &BranchAgent<'_>,
    memo_slot: Option<u32>,
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
    emit_agent_plan(
        body,
        indices,
        static_data,
        track_events,
        variables,
        branch.step_id,
        branch.agent_id,
        branch.agent_component_id,
        branch.input_mapping_id,
        // Durable branches replay via the standard durable block; the launch gate
        // (below) prevents a replay double-fire. The durable key ignores
        // source.steps, so it matches across launch (fan-out source) and assemble
        // (sibling-accumulating source).
        branch.durable_checkpoint,
        false, // breakpoint (excluded by eligibility)
        // Retries run in assemble (memoized attempt 1 + sequential backoff), like
        // the Split window's non-concurrent-backoff path.
        branch.max_retries,
        branch.retry_delay_ms,
        branch.rate_limit_budget_ms,
        branch.next_plan, // Join — emits nothing; the merge runs once after
        branch.error_plan,
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
        memo_slot,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_parallel_branches(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    branches: &[DirectRunPlan],
    merge_plan: &DirectRunPlan,
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
    let concurrent = static_data.parallel_enabled
        && branches
            .iter()
            .all(|branch| branch_concurrent_eligible(static_data, branch));

    if concurrent {
        emit_concurrent_branches(
            body,
            indices,
            static_data,
            track_events,
            variables,
            branches,
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
    } else {
        // Sequential fallback: the exact linearised instruction stream — each
        // branch agent in order (no window, no memo), then the merge.
        for branch in branches {
            emit_branch_agent(
                body,
                indices,
                static_data,
                track_events,
                variables,
                &branch_agent(branch),
                None,
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

    // Shared continuation: emitted ONCE, at the base block depth (all branches
    // reached their `Join`), exactly like `Conditional`/`SwitchRoute`.
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

#[allow(clippy::too_many_arguments)]
fn emit_concurrent_branches(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    branches: &[DirectRunPlan],
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
    let ws_new = indices
        .waitable_set_new
        .expect("parallel-branch compiles import the waitable builtins");
    let ws_wait = indices
        .waitable_set_wait
        .expect("parallel-branch compiles import the waitable builtins");
    let ws_drop = indices
        .waitable_set_drop
        .expect("parallel-branch compiles import the waitable builtins");
    let waitable_join = indices
        .waitable_join
        .expect("parallel-branch compiles import the waitable builtins");
    let subtask_drop = indices
        .subtask_drop
        .expect("parallel-branch compiles import the waitable builtins");

    let branch_count = branches.len() as i32;
    let slots_bytes = branch_count * DIRECT_PSPLIT_SLOT_STRIDE;

    // slots = bump(align8(global0), branch_count * STRIDE), zero-filled. The slot
    // retptrs receive canonical-ABI stores that require natural alignment, so the
    // base is aligned to 8 off the byte-granular bump pointer.
    body.instruction(&Instruction::GlobalGet(0));
    body.instruction(&Instruction::I32Const(7));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::I32Const(-8));
    body.instruction(&Instruction::I32And);
    body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_SLOTS_LOCAL));
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_SLOTS_LOCAL));
    body.instruction(&Instruction::I32Const(slots_bytes));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::GlobalSet(0));
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_SLOTS_LOCAL));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Const(slots_bytes));
    body.instruction(&Instruction::MemoryFill(0));

    // ws = waitable-set.new(); pending = 0; signal = 0.
    body.instruction(&Instruction::Call(ws_new));
    body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_WS_LOCAL));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_PENDING_LOCAL));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_SIGNAL_LOCAL));

    // Assign each branch a pool member so branches sharing a component get
    // DISTINCT instances (a sync-lifted instance serializes concurrent entries on
    // its lock). member = occurrence-among-same-component % pool_size; with
    // pool_size = min(count, PARALLEL_POOL_MAX), branches ≤ MAX get unique members
    // and overlap fully; any excess wraps (round-robin) and serializes.
    let mut counts: BTreeMap<&str, u32> = BTreeMap::new();
    for branch in branches {
        *counts
            .entry(branch_agent(branch).agent_component_id)
            .or_insert(0) += 1;
    }
    let mut occurrence: BTreeMap<&str, u32> = BTreeMap::new();
    let members: Vec<u32> = branches
        .iter()
        .map(|branch| {
            let component = branch_agent(branch).agent_component_id;
            let slot = occurrence.entry(component).or_insert(0);
            let member = *slot % pool_size_for_window(counts[component]);
            *slot += 1;
            member
        })
        .collect();

    // ── LAUNCH pass (unrolled per branch) ───────────────────────────────────
    for (index, branch) in branches.iter().enumerate() {
        emit_branch_launch(
            body,
            indices,
            static_data,
            &branch_agent(branch),
            index as i32,
            members[index],
            waitable_join,
            source_ptr_local,
            source_len_local,
            output_ptr_local,
            output_len_local,
            route_ptr_local,
            route_len_local,
        );
    }

    // ── DRAIN ────────────────────────────────────────────────────────────────
    emit_drain_pending(body, indices, ws_wait, subtask_drop);
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_WS_LOCAL));
    body.instruction(&Instruction::Call(ws_drop));

    // ── ASSEMBLE pass (unrolled per branch, in order) ────────────────────────
    for (index, branch) in branches.iter().enumerate() {
        // memo slot for this branch: slots + index * STRIDE -> launch scratch.
        body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_SLOTS_LOCAL));
        body.instruction(&Instruction::I32Const(
            index as i32 * DIRECT_PSPLIT_SLOT_STRIDE,
        ));
        body.instruction(&Instruction::I32Add);
        body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_LAUNCH_LOCAL));

        emit_branch_agent(
            body,
            indices,
            static_data,
            track_events,
            variables,
            &branch_agent(branch),
            Some(DIRECT_PSPLIT_LAUNCH_LOCAL),
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

    // Act on a pause/cancel flagged during the drain. Every subtask has resolved
    // and assemble has run, so this is a replay-safe suspend point — mirror the
    // Split chunk boundary.
    if !indices.omit_runtime {
        body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_SIGNAL_LOCAL));
        body.instruction(&Instruction::If(BlockType::Empty));
        for poll in [indices.runtime_is_cancelled, indices.runtime_check_signals] {
            push_retptr_arg(body);
            body.instruction(&Instruction::Call(poll));
            emit_retptr_error_or_return(body, indices, None, route_ptr_local, route_len_local);
            push_retptr_u8_load(body, DIRECT_RET_BOOL_OK_OFFSET);
            body.instruction(&Instruction::If(BlockType::Empty));
            emit_entry_suspend_return(body, indices);
            body.instruction(&Instruction::End);
        }
        body.instruction(&Instruction::I32Const(0));
        body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_SIGNAL_LOCAL));
        body.instruction(&Instruction::End);
    }
}

/// Launch one branch's async-lowered invoke into its slot. Best-effort: any input
/// preparation failure (mapping, validation, connection) skips the launch — the
/// slot stays EMPTY and assemble reproduces the exact failure through the standard
/// Agent lowering.
#[allow(clippy::too_many_arguments)]
fn emit_branch_launch(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    branch: &BranchAgent<'_>,
    index: i32,
    pool_member: u32,
    waitable_join: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    output_ptr_local: u32,
    output_len_local: u32,
    route_ptr_local: u32,
    route_len_local: u32,
) {
    let component_id = pool_member_component_id(branch.agent_component_id, pool_member);
    let invoke = indices
        .agent_invokes_async
        .get(&component_id)
        .expect("parallel branch agents have matching async pool imports");
    let capability_id = static_data
        .agent_capability_id(branch.agent_id)
        .expect("parallel branch agents have static capability ids");

    // slot_ptr = slots + index * STRIDE -> launch scratch.
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_SLOTS_LOCAL));
    body.instruction(&Instruction::I32Const(index * DIRECT_PSPLIT_SLOT_STRIDE));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_LAUNCH_LOCAL));

    body.instruction(&Instruction::Block(BlockType::Empty)); // $skip
    let skip_on_error = |body: &mut WasmFunction| {
        load_retptr_tag(body);
        body.instruction(&Instruction::BrIf(0));
    };

    // Durable: gate the launch on the step-level checkpoint. A HIT (resumed run)
    // means the agent already ran on a prior life — skip the launch so it never
    // double-fires; assemble's durable block replays the stored result. The key is
    // computed from the fan-out source, but the durable key ignores `source.steps`,
    // so it matches the assemble key despite sibling accumulation.
    if branch.durable_checkpoint {
        body.instruction(&Instruction::I32Const(branch.agent_id as i32));
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
        emit_get_checkpoint_has_value(body);
        body.instruction(&Instruction::BrIf(0)); // HIT -> skip launch
    }

    // agent input = apply-mapping(mapping_id, source). All branches share the
    // fan-out point's source (each sees only its predecessors' context).
    body.instruction(&Instruction::I32Const(branch.input_mapping_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_apply_mapping));
    skip_on_error(body);
    load_retptr_list(body, output_ptr_local, output_len_local);

    // agent-validate-input: a NON-EMPTY ok payload is a validation error — skip
    // (assemble reproduces it with full debug/error routing).
    body.instruction(&Instruction::I32Const(branch.agent_id as i32));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_validate_input));
    skip_on_error(body);
    load_retptr_list(body, route_ptr_local, route_len_local);
    body.instruction(&Instruction::LocalGet(route_len_local));
    body.instruction(&Instruction::BrIf(0)); // -> $skip

    // connection injection (in-band `_connection`); no-op when connectionless.
    if static_data.agent_has_connection(branch.agent_id) {
        body.instruction(&Instruction::I32Const(branch.agent_id as i32));
        body.instruction(&Instruction::LocalGet(output_ptr_local));
        body.instruction(&Instruction::LocalGet(output_len_local));
        body.instruction(&Instruction::LocalGet(source_ptr_local));
        body.instruction(&Instruction::LocalGet(source_len_local));
        push_retptr_arg(body);
        body.instruction(&Instruction::Call(indices.stdlib_agent_connection_input));
        skip_on_error(body);
        load_retptr_list(body, output_ptr_local, output_len_local);
    }

    // slot.state = AGENT_READY, then async-invoke into slot+RESULT_OFFSET.
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_LAUNCH_LOCAL));
    body.instruction(&Instruction::I32Const(SLOT_AGENT_READY));
    body.instruction(&Instruction::I32Store(mem32()));

    push_segment_args(body, capability_id);
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_LAUNCH_LOCAL));
    body.instruction(&Instruction::I32Const(DIRECT_PSPLIT_SLOT_RESULT_OFFSET));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::Call(invoke.function_index));
    body.instruction(&Instruction::LocalSet(route_len_local)); // status
    emit_join_if_pending(body, route_len_local, waitable_join);

    body.instruction(&Instruction::End); // $skip
}
