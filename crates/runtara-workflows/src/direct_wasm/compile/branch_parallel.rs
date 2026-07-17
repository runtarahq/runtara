// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Concurrent parallel-branch lowering (docs/wasip3-parallel-branches-plan.md,
//! Phases 4a + 4b).
//!
//! An unconditional fan-out `A → {branch, branch, …} → M` runs the branches
//! CONCURRENTLY instead of linearising them. A branch is a linear Agent chain
//! (4a: length 1; 4b: length N), run as a DEPTH-WAVEFRONT — at each depth the
//! depth-d step of every branch launches together, drains, and assembles into the
//! shared `steps` context that depth d+1 reads (§4.0). Per depth:
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

/// Borrowed fields of an Agent chain node. The wavefront supplies `next_plan`
/// explicitly (`Join`), so it is not carried here.
struct BranchAgent<'a> {
    step_id: &'a str,
    agent_id: u32,
    agent_component_id: &'a str,
    input_mapping_id: u32,
    durable_checkpoint: bool,
    max_retries: u32,
    retry_delay_ms: u64,
    rate_limit_budget_ms: u64,
    error_plan: Option<&'a DirectErrorRoutePlan>,
}

/// Extract an Agent chain node's fields. Only called for `Agent` nodes (the
/// wavefront dispatches sync nodes elsewhere).
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
        error_plan,
        ..
    } = plan
    else {
        unreachable!("branch_agent called on a non-Agent chain node");
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
        error_plan: error_plan.as_ref(),
    }
}

/// Walk a branch plan into its linear chain of nodes `[s0, s1, …]` — Agents (4a/4b)
/// interleaved with SYNC non-Agent steps (4c.1: Log/Filter/SwitchValue/GroupBy) —
/// stopping before `Join`. `plan.rs::is_linear_chain_branch` guarantees each node
/// is a supported linear type.
fn branch_chain(plan: &DirectRunPlan) -> Vec<&DirectRunPlan> {
    let mut chain = Vec::new();
    let mut node = plan;
    loop {
        chain.push(node);
        match chain_next(node) {
            Some(next) if !matches!(next, DirectRunPlan::Join) => node = next,
            _ => break,
        }
    }
    chain
}

/// The continuation of a chain node — `next_plan` for linear nodes, `merge_plan`
/// for an in-branch Conditional composite (4c.3) — else `None`.
fn chain_next(node: &DirectRunPlan) -> Option<&DirectRunPlan> {
    match node {
        DirectRunPlan::Agent { next_plan, .. }
        | DirectRunPlan::Log { next_plan, .. }
        | DirectRunPlan::Filter { next_plan, .. }
        | DirectRunPlan::SwitchValue { next_plan, .. }
        | DirectRunPlan::GroupBy { next_plan, .. } => Some(next_plan),
        DirectRunPlan::Conditional { merge_plan, .. } => merge_plan.as_deref(),
        _ => None,
    }
}

/// A sync chain node has no async op, so the wavefront runs it in assemble via
/// the standard dispatcher — but with its `next_plan` replaced by `Join`, so ONLY
/// this step emits (its successor runs at the next depth). Agents go through
/// `emit_branch_agent` (memoized) instead and never reach here.
fn with_next_join(node: &DirectRunPlan) -> DirectRunPlan {
    let next_plan = Box::new(DirectRunPlan::Join);
    match node {
        DirectRunPlan::Log {
            step_id,
            log_id,
            breakpoint,
            ..
        } => DirectRunPlan::Log {
            step_id: step_id.clone(),
            log_id: *log_id,
            breakpoint: *breakpoint,
            next_plan,
        },
        DirectRunPlan::Filter {
            step_id,
            filter_id,
            breakpoint,
            ..
        } => DirectRunPlan::Filter {
            step_id: step_id.clone(),
            filter_id: *filter_id,
            breakpoint: *breakpoint,
            next_plan,
        },
        DirectRunPlan::SwitchValue {
            step_id,
            switch_id,
            breakpoint,
            ..
        } => DirectRunPlan::SwitchValue {
            step_id: step_id.clone(),
            switch_id: *switch_id,
            breakpoint: *breakpoint,
            next_plan,
        },
        DirectRunPlan::GroupBy {
            step_id,
            group_id,
            breakpoint,
            ..
        } => DirectRunPlan::GroupBy {
            step_id: step_id.clone(),
            group_id: *group_id,
            breakpoint: *breakpoint,
            next_plan,
        },
        // Composite: run the whole conditional blocking (its arms end in Join at
        // the internal merge); its post-merge continuation becomes Join so only
        // this composite emits — the real continuation runs at the next depth.
        DirectRunPlan::Conditional {
            step_id,
            condition_id,
            breakpoint,
            true_plan,
            false_plan,
            ..
        } => DirectRunPlan::Conditional {
            step_id: step_id.clone(),
            condition_id: *condition_id,
            breakpoint: *breakpoint,
            true_plan: true_plan.clone(),
            false_plan: false_plan.clone(),
            merge_plan: Some(next_plan),
        },
        other => other.clone(),
    }
}

/// `Some(pool_sizes)` when the whole fan-out may run concurrently — async ABI
/// (`parallel_enabled`) and no chain step targets a workflow-agent (shared runtime
/// host / checkpoint scope, a Phase-4c question) — else `None` (sequential
/// fallback). Returned pool sizes drive BOTH the emitter's member assignment and
/// the `[async-lower]invoke` imports the composer emits, so they cannot disagree.
pub(super) fn concurrent_branch_pools(
    static_data: &DirectCoreStaticData,
    branches: &[DirectRunPlan],
) -> Option<BTreeMap<String, u32>> {
    if !static_data.parallel_enabled {
        return None;
    }
    let chains: Vec<Vec<&DirectRunPlan>> = branches.iter().map(branch_chain).collect();
    let ok = chains.iter().flatten().all(|node| match node {
        DirectRunPlan::Agent { agent_id, .. } => !static_data.agent_is_workflow_agent(*agent_id),
        _ => true, // sync steps have no invoke
    });
    if !ok {
        return None;
    }
    let (_, pool_sizes) = plan_branch_pools(&chains);
    Some(pool_sizes)
}

/// Per-branch, per-depth pool member indices + the pool size per component. In
/// the depth-wavefront only steps invoked at the SAME depth run concurrently, so
/// a component contends only with same-depth peers: its pool = the max number of
/// branches invoking it at any single depth (clamped to PARALLEL_POOL_MAX). A
/// component reused across depths (sequential rounds) needs no extra instance.
#[allow(clippy::needless_range_loop)] // depth indexes chains, raw, and members in parallel
fn plan_branch_pools(chains: &[Vec<&DirectRunPlan>]) -> (Vec<Vec<u32>>, BTreeMap<String, u32>) {
    // Only Agent nodes invoke; sync steps get member 0 (unused).
    let component_of = |node: &DirectRunPlan| -> Option<String> {
        match node {
            DirectRunPlan::Agent {
                agent_component_id, ..
            } => Some(agent_component_id.clone()),
            _ => None,
        }
    };
    let max_depth = chains.iter().map(Vec::len).max().unwrap_or(0);
    let mut raw: Vec<Vec<u32>> = chains.iter().map(|c| vec![0u32; c.len()]).collect();
    let mut pool_max: BTreeMap<String, u32> = BTreeMap::new();
    for depth in 0..max_depth {
        let mut per_depth: BTreeMap<String, u32> = BTreeMap::new();
        for (branch, chain) in chains.iter().enumerate() {
            if let Some(component) = chain.get(depth).and_then(|node| component_of(node)) {
                let count = per_depth.entry(component).or_insert(0);
                raw[branch][depth] = *count;
                *count += 1;
            }
        }
        for (component, count) in per_depth {
            let entry = pool_max.entry(component).or_insert(0);
            *entry = (*entry).max(count);
        }
    }
    let pool_sizes: BTreeMap<String, u32> = pool_max
        .iter()
        .map(|(component, count)| (component.clone(), pool_size_for_window(*count)))
        .collect();
    let members: Vec<Vec<u32>> = chains
        .iter()
        .enumerate()
        .map(|(branch, chain)| {
            (0..chain.len())
                .map(
                    |depth| match chain.get(depth).and_then(|node| component_of(node)) {
                        Some(component) => {
                            let size = pool_sizes.get(&component).copied().unwrap_or(1);
                            raw[branch][depth] % size
                        }
                        None => 0,
                    },
                )
                .collect()
        })
        .collect();
    (members, pool_sizes)
}

/// Emit one branch STEP's Agent lowering in the assemble pass, with the invoke
/// memoized from `memo_slot`. `next_plan` is `Join` in the wavefront (the next
/// chain step is handled at the next depth; the merge runs once at the end).
#[allow(clippy::too_many_arguments)]
fn emit_branch_agent(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    branch: &BranchAgent<'_>,
    next_plan: &DirectRunPlan,
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
        next_plan,
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
    let concurrent = concurrent_branch_pools(static_data, branches).is_some();

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
        // branch's FULL chain in order (no window, no memo; emit_agent_plan
        // recurses through the chain to its Join), then the merge.
        for branch in branches {
            emit_run_plan_mapping(
                body,
                indices,
                static_data,
                track_events,
                variables,
                branch,
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
#[allow(clippy::needless_range_loop)] // depth indexes chains, members, and slot offsets in parallel
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

    // Walk each branch into its chain (Agents + sync steps) and assign per-depth
    // pool members (Agent nodes only).
    let chains: Vec<Vec<&DirectRunPlan>> = branches.iter().map(branch_chain).collect();
    let (members, _pool_sizes) = plan_branch_pools(&chains);
    let max_depth = chains.iter().map(Vec::len).max().unwrap_or(0);

    let branch_count = branches.len() as i32;
    let slots_bytes = branch_count * DIRECT_PSPLIT_SLOT_STRIDE;

    // slots = bump(align8(global0), branch_count * STRIDE), zero-filled — K slots
    // (one per branch), REUSED each depth of the wavefront. The slot retptrs
    // receive canonical-ABI stores that require natural alignment, so the base is
    // aligned to 8 off the byte-granular bump pointer.
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

    // signal = 0 (accumulated across every depth's drain; the suspend fires once
    // after the window quiesces).
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_SIGNAL_LOCAL));

    // ── DEPTH-WAVEFRONT ──────────────────────────────────────────────────────
    // At each depth, launch → drain → assemble the depth-d step of every branch
    // that still has one. Assemble runs each step with `next = Join` (the next
    // chain step is handled at the next depth) and MEMOIZES its invoke; its output
    // lands in the SHARED `steps` context that depth d+1 reads. A length-1 chain
    // (4a) is exactly one depth. (docs/wasip3-parallel-branches-plan.md §4.0.)
    let join = DirectRunPlan::Join;
    for depth in 0..max_depth {
        // Fresh waitable-set + pending for this depth; slots are reused (assemble
        // consumes each, resetting its state).
        body.instruction(&Instruction::Call(ws_new));
        body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_WS_LOCAL));
        body.instruction(&Instruction::I32Const(0));
        body.instruction(&Instruction::LocalSet(DIRECT_PSPLIT_PENDING_LOCAL));

        // LAUNCH depth d — only Agent steps have an async invoke; sync steps
        // (Log/Filter/…) run in assemble with no launch.
        for (index, chain) in chains.iter().enumerate() {
            if let Some(node) = chain.get(depth)
                && matches!(node, DirectRunPlan::Agent { .. })
            {
                emit_branch_launch(
                    body,
                    indices,
                    static_data,
                    &branch_agent(node),
                    index as i32,
                    members[index][depth],
                    waitable_join,
                    source_ptr_local,
                    source_len_local,
                    output_ptr_local,
                    output_len_local,
                    route_ptr_local,
                    route_len_local,
                );
            }
        }

        // DRAIN depth d.
        emit_drain_pending(body, indices, ws_wait, subtask_drop);
        body.instruction(&Instruction::LocalGet(DIRECT_PSPLIT_WS_LOCAL));
        body.instruction(&Instruction::Call(ws_drop));

        // ASSEMBLE depth d (branch order): Agent steps consume their memoized
        // slot; sync steps run through the standard dispatcher, one step only
        // (`next = Join`), updating the shared context depth d+1 reads.
        for (index, chain) in chains.iter().enumerate() {
            if let Some(node) = chain.get(depth) {
                if matches!(node, DirectRunPlan::Agent { .. }) {
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
                        &branch_agent(node),
                        &join,
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
                } else {
                    let single = with_next_join(node);
                    emit_run_plan_mapping(
                        body,
                        indices,
                        static_data,
                        track_events,
                        variables,
                        &single,
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
        }
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
