// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agent step lowering for the direct workflow core Wasm emitter.
//!
//! The convergence point for every flavour of capability invocation.
//! `emit_agent_plan` applies the input mapping, validates input, injects stored
//! connection params, then invokes the agent component — optionally wrapped in a
//! retry/backoff loop and/or a durable-checkpoint `if/else` (a cache hit
//! short-circuits the invoke). Errors route through the shared onError-or-fail
//! path; success shapes the step output and chains into `next_plan`. An `output_fn`
//! parameter lets a single-shot `AiAgent` step reuse this entire invoke/retry/
//! checkpoint machinery while only swapping the final result-envelope call, so
//! those concerns aren't duplicated per step kind.

use wasm_encoder::{BlockType, Function as WasmFunction, Instruction};

use super::abi::{
    emit_get_checkpoint_has_value, emit_retptr_error_or_return, load_agent_retptr_list,
    load_retptr_list, load_retptr_option_list, load_retptr_tag, push_retptr_arg,
    return_if_retptr_error,
};
use super::agent_error::{
    emit_agent_error_route_or_fail, emit_agent_invoke_error_body_from_info,
    emit_agent_invoke_error_branch,
};
use super::agent_invoke::emit_agent_invoke;
use super::agent_io::{emit_agent_cache_key, emit_agent_scope_input};
use super::agent_retry::{
    emit_agent_advance_retry_attempt, emit_agent_attempt_decode, emit_agent_capture_retry_sleep,
    emit_agent_record_retry_attempt, emit_agent_retry_condition, emit_agent_retry_delay,
    emit_agent_retry_error_info, emit_agent_retry_sleep,
};
use super::checkpoint::{emit_checkpoint_lookup, emit_checkpoint_save};
use super::debug::{
    emit_agent_debug_error, emit_step_breakpoint, emit_step_debug_end_timed, emit_step_debug_event,
};
use super::dispatcher::emit_run_plan_mapping;
use super::mapping::{emit_apply_mapping_start_step_error, emit_build_source};
use super::{
    DIRECT_AGENT_ATTEMPT_ENV_LEN_LOCAL, DIRECT_AGENT_ATTEMPT_ENV_PTR_LOCAL,
    DIRECT_AGENT_ATTEMPT_ERR_FLAG_LOCAL, DIRECT_AGENT_ATTEMPT_HIT_FLAG_LOCAL,
    DIRECT_AGENT_ATTEMPT_KEY_LEN_LOCAL, DIRECT_AGENT_ATTEMPT_KEY_PTR_LOCAL,
    DIRECT_AGENT_RATE_LIMITED_LOCAL, DIRECT_AGENT_RETRY_ATTEMPT_LOCAL,
    DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL, DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL,
    DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL, DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL,
    DIRECT_AGENT_RETRYABLE_LOCAL, DirectCoreFunctionIndices, DirectCoreStaticData,
    DirectDataSegment, DirectErrorRoutePlan, DirectFailureTarget, DirectHandledTarget,
    DirectRunPlan, DirectVariables,
};

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_agent_plan(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    variables: DirectVariables<'_>,
    step_id: &str,
    agent_id: u32,
    agent_component_id: &str,
    input_mapping_id: u32,
    durable_checkpoint: bool,
    breakpoint: bool,
    max_retries: u32,
    retry_delay_ms: u64,
    rate_limit_budget_ms: u64,
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
    // stdlib function index used to build the step output context from the
    // capability result: `agent-output` for a normal Agent step, or
    // `ai-agent-output` for an AiAgent step (which transforms the
    // `chat-completion` choice into the `{response, iterations, toolCalls}`
    // envelope). Both share the rest of the invoke/checkpoint/retry path.
    output_fn: u32,
    // Parallel-split memoization (docs/wasip3-parallelism.md Phase 3): a local
    // holding this item's slot pointer. When the slot's state is non-zero the
    // launch pass already ran the invoke — its canonical result is copied from
    // the slot to the retptr scratch instead of re-invoking; an EMPTY slot
    // falls back to the synchronous invoke. Only reachable on the
    // no-retry/non-durable path (parallel eligibility excludes the others).
    memo_slot_ptr_local: Option<u32>,
) {
    // Resolve the input mapping. An unhandled failure (e.g. a template render
    // error) is attributed to this step — a start + error step-debug pair — so
    // its per-step record carries the error and a duration instead of the failure
    // surfacing only at execution level; an onError handler routes as before.
    // The Agent step's start event fires after its mapping, so the failure path
    // emits its own start (see `emit_apply_mapping_start_step_error`).
    emit_apply_mapping_start_step_error(
        body,
        indices,
        static_data,
        track_events,
        input_mapping_id,
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
        route_ptr_local,
        route_len_local,
    );

    emit_agent_input_validation(
        body,
        indices,
        static_data,
        track_events,
        agent_id,
        step_id,
        output_ptr_local,
        output_len_local,
        source_ptr_local,
        source_len_local,
        steps_ptr_local,
        steps_len_local,
        error_plan,
        route_ptr_local,
        route_len_local,
        variables,
        data_ptr_local,
        data_len_local,
        workflow_log_kind,
        workflow_error_kind,
        failure_target,
        handled_target,
    );

    // The connection is injected into the input at the invoke boundary
    // (`emit_agent_invoke` → `emit_agent_connection_input`), so it is resolved
    // once, per invoke, for every agent kind — nothing to do here.

    // A workflow-agent child shares this instance's checkpoint store: wrap its
    // input in the `{data, variables}` envelope carrying the invocation-site
    // namespace so the child's durable keys never collide with the parent's or
    // another invocation's. Once, here — before the retry loop (every attempt
    // reuses the wrapped buffer) and before the durable cache-key block (which
    // reads `source`, not this buffer). No-op for native agents.
    emit_agent_scope_input(
        body,
        indices,
        static_data,
        agent_id,
        output_ptr_local,
        output_len_local,
        source_ptr_local,
        source_len_local,
    );

    if durable_checkpoint {
        emit_agent_cache_key(
            body,
            indices,
            agent_id,
            source_ptr_local,
            source_len_local,
            route_ptr_local,
            route_len_local,
        );
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

    let invoke = indices
        .agent_invokes
        .get(agent_component_id)
        .expect("direct Agent run plans have matching component imports");
    let capability_id = static_data
        .agent_capability_id(agent_id)
        .expect("direct Agent run plans have static capability ids");
    if max_retries > 0 {
        body.instruction(&Instruction::I32Const(1));
        body.instruction(&Instruction::LocalSet(DIRECT_AGENT_RETRY_ATTEMPT_LOCAL));
        body.instruction(&Instruction::Block(BlockType::Empty));
        body.instruction(&Instruction::Loop(BlockType::Empty));

        // Per-attempt prologue: run this attempt fresh, or replay it from a
        // per-attempt checkpoint. On a durable step every FAILED attempt is
        // checkpointed under `{cache_key}::attempt::{N}` before the backoff, so a
        // resume (replay-from-start after a drain/restart mid-retry) short-circuits
        // the invoke for attempts that already ran instead of re-firing the agent —
        // the bug this fixes. Both paths leave `ATTEMPT_ERR_FLAG` set and, on a
        // failure, the retry state-machine locals populated.
        if durable_checkpoint {
            // 1. Build the per-attempt result key: `{cache_key}::attempt::{N}`.
            body.instruction(&Instruction::LocalGet(route_ptr_local));
            body.instruction(&Instruction::LocalGet(route_len_local));
            body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_ATTEMPT_LOCAL));
            push_retptr_arg(body);
            body.instruction(&Instruction::Call(indices.stdlib_agent_attempt_result_key));
            return_if_retptr_error(body, indices);
            load_retptr_list(
                body,
                DIRECT_AGENT_ATTEMPT_KEY_PTR_LOCAL,
                DIRECT_AGENT_ATTEMPT_KEY_LEN_LOCAL,
            );

            // 2. Read-only lookup of this attempt's checkpoint -> HIT_FLAG.
            body.instruction(&Instruction::LocalGet(DIRECT_AGENT_ATTEMPT_KEY_PTR_LOCAL));
            body.instruction(&Instruction::LocalGet(DIRECT_AGENT_ATTEMPT_KEY_LEN_LOCAL));
            push_retptr_arg(body);
            body.instruction(&Instruction::Call(indices.runtime_get_checkpoint));
            emit_get_checkpoint_has_value(body);
            body.instruction(&Instruction::LocalSet(DIRECT_AGENT_ATTEMPT_HIT_FLAG_LOCAL));

            body.instruction(&Instruction::LocalGet(DIRECT_AGENT_ATTEMPT_HIT_FLAG_LOCAL));
            body.instruction(&Instruction::If(BlockType::Empty));
            // HIT: a stored failure. Decode it into the retry locals — no invoke.
            load_retptr_option_list(
                body,
                DIRECT_AGENT_ATTEMPT_ENV_PTR_LOCAL,
                DIRECT_AGENT_ATTEMPT_ENV_LEN_LOCAL,
            );
            emit_agent_attempt_decode(
                body,
                DIRECT_AGENT_ATTEMPT_ENV_PTR_LOCAL,
                DIRECT_AGENT_ATTEMPT_ENV_LEN_LOCAL,
            );
            body.instruction(&Instruction::Else);
            // MISS: this attempt has not run. Invoke, then persist its outcome.
            if let Some(slot_ptr_local) = memo_slot_ptr_local {
                // Parallel-split memoization (consume-once): the launch pass's
                // speculative result substitutes for THIS attempt's invoke;
                // later attempts find the slot consumed and re-invoke.
                body.instruction(&Instruction::LocalGet(slot_ptr_local));
                body.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                body.instruction(&Instruction::If(BlockType::Empty));
                body.instruction(&Instruction::I32Const(0));
                body.instruction(&Instruction::LocalGet(slot_ptr_local));
                body.instruction(&Instruction::I32Const(
                    super::DIRECT_PSPLIT_SLOT_RESULT_OFFSET,
                ));
                body.instruction(&Instruction::I32Add);
                body.instruction(&Instruction::I32Const(super::DIRECT_PSPLIT_SLOT_RESULT_LEN));
                body.instruction(&Instruction::MemoryCopy {
                    src_mem: 0,
                    dst_mem: 0,
                });
                body.instruction(&Instruction::LocalGet(slot_ptr_local));
                body.instruction(&Instruction::I32Const(0));
                body.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                body.instruction(&Instruction::Else);
            }
            emit_agent_invoke(
                body,
                indices,
                invoke,
                capability_id,
                static_data,
                agent_id,
                output_ptr_local,
                output_len_local,
                source_ptr_local,
                source_len_local,
            );
            if memo_slot_ptr_local.is_some() {
                body.instruction(&Instruction::End);
            }
            load_retptr_tag(body);
            body.instruction(&Instruction::LocalSet(DIRECT_AGENT_ATTEMPT_ERR_FLAG_LOCAL));
            body.instruction(&Instruction::LocalGet(DIRECT_AGENT_ATTEMPT_ERR_FLAG_LOCAL));
            body.instruction(&Instruction::If(BlockType::Empty));
            // Fresh failure: capture the classification + error payload BEFORE the
            // state machine runs (its sleep path reuses the error locals as
            // scratch), then checkpoint the per-attempt envelope. On a successful
            // attempt (ERR_FLAG == 0) nothing is stored here — the outer step
            // checkpoint covers success, keeping the write cost to failures only.
            emit_agent_capture_retry_sleep(body);
            emit_agent_retry_error_info(
                body,
                indices,
                DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL,
                DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL,
            );
            // Encode {tag=err, retryable, rate_limited, retry_after_tag,
            // retry_after_ms(raw), payload}. Persisting the already-computed
            // classification bits (not re-deriving on replay) is required: the
            // agent retry formula differs from the workflow one and the latter
            // reads AUTO_RETRY_ON_429 from the environment at classify time.
            body.instruction(&Instruction::I32Const(1));
            body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRYABLE_LOCAL));
            body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RATE_LIMITED_LOCAL));
            body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_TAG_LOCAL));
            body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_SLEEP_MS_LOCAL));
            body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL));
            body.instruction(&Instruction::LocalGet(DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL));
            push_retptr_arg(body);
            body.instruction(&Instruction::Call(indices.stdlib_agent_attempt_envelope));
            return_if_retptr_error(body, indices);
            load_retptr_list(
                body,
                DIRECT_AGENT_ATTEMPT_ENV_PTR_LOCAL,
                DIRECT_AGENT_ATTEMPT_ENV_LEN_LOCAL,
            );
            // Bare `checkpoint` (not `emit_checkpoint_save`): a durability point,
            // not a suspend point — `handle_checkpoint` is load-first idempotent,
            // and the following backoff sleep is where a pending cancel/pause parks
            // the instance. Its `checkpoint-result` (found / pending-signal) is
            // intentionally ignored here.
            body.instruction(&Instruction::LocalGet(DIRECT_AGENT_ATTEMPT_KEY_PTR_LOCAL));
            body.instruction(&Instruction::LocalGet(DIRECT_AGENT_ATTEMPT_KEY_LEN_LOCAL));
            body.instruction(&Instruction::LocalGet(DIRECT_AGENT_ATTEMPT_ENV_PTR_LOCAL));
            body.instruction(&Instruction::LocalGet(DIRECT_AGENT_ATTEMPT_ENV_LEN_LOCAL));
            push_retptr_arg(body);
            body.instruction(&Instruction::Call(indices.runtime_checkpoint));
            return_if_retptr_error(body, indices);
            body.instruction(&Instruction::End); // fresh-failure If
            body.instruction(&Instruction::End); // hit/miss If
        } else {
            // Non-durable: no per-attempt durability. `HIT_FLAG` is never set (stays
            // 0), so the shared sleep gate below always sleeps — identical behavior
            // to before this change.
            //
            // Parallel-split memoization: attempt 1 consumes the launch pass's
            // slot result (see the no-retry site for the layout); attempts 2+
            // find the slot consumed and re-invoke synchronously.
            if let Some(slot_ptr_local) = memo_slot_ptr_local {
                body.instruction(&Instruction::LocalGet(slot_ptr_local));
                body.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                body.instruction(&Instruction::If(BlockType::Empty));
                body.instruction(&Instruction::I32Const(0)); // dst: retptr scratch
                body.instruction(&Instruction::LocalGet(slot_ptr_local));
                body.instruction(&Instruction::I32Const(
                    super::DIRECT_PSPLIT_SLOT_RESULT_OFFSET,
                ));
                body.instruction(&Instruction::I32Add);
                body.instruction(&Instruction::I32Const(super::DIRECT_PSPLIT_SLOT_RESULT_LEN));
                body.instruction(&Instruction::MemoryCopy {
                    src_mem: 0,
                    dst_mem: 0,
                });
                // Consume-once: a retry attempt must re-invoke, not replay the
                // same memoized outcome.
                body.instruction(&Instruction::LocalGet(slot_ptr_local));
                body.instruction(&Instruction::I32Const(0));
                body.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                    offset: 0,
                    align: 2,
                    memory_index: 0,
                }));
                body.instruction(&Instruction::Else);
            }
            emit_agent_invoke(
                body,
                indices,
                invoke,
                capability_id,
                static_data,
                agent_id,
                output_ptr_local,
                output_len_local,
                source_ptr_local,
                source_len_local,
            );
            if memo_slot_ptr_local.is_some() {
                body.instruction(&Instruction::End);
            }
            load_retptr_tag(body);
            body.instruction(&Instruction::LocalSet(DIRECT_AGENT_ATTEMPT_ERR_FLAG_LOCAL));
            body.instruction(&Instruction::LocalGet(DIRECT_AGENT_ATTEMPT_ERR_FLAG_LOCAL));
            body.instruction(&Instruction::If(BlockType::Empty));
            emit_agent_capture_retry_sleep(body);
            emit_agent_retry_error_info(
                body,
                indices,
                DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL,
                DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL,
            );
            body.instruction(&Instruction::End);
        }

        // Shared retry state machine, driven by ATTEMPT_ERR_FLAG (set by both the
        // fresh-invoke and the checkpoint-replay paths). On the success path
        // (ERR_FLAG == 0) this whole block is skipped and the invoke's OK result —
        // still in the retptr, only ever reached on a fresh MISS — is materialized.
        body.instruction(&Instruction::LocalGet(DIRECT_AGENT_ATTEMPT_ERR_FLAG_LOCAL));
        body.instruction(&Instruction::If(BlockType::Empty));
        emit_agent_retry_condition(body, max_retries, retry_delay_ms, rate_limit_budget_ms);
        body.instruction(&Instruction::If(BlockType::Empty));
        emit_agent_advance_retry_attempt(body);
        if durable_checkpoint {
            // A replayed (HIT) attempt already slept its backoff and recorded its
            // audit row on the original run; skip both. Core `handle_sleep`
            // re-sleeps the full duration on replay, so this gate — not the sleep
            // key — is what prevents re-sleeping every completed attempt.
            body.instruction(&Instruction::LocalGet(DIRECT_AGENT_ATTEMPT_HIT_FLAG_LOCAL));
            body.instruction(&Instruction::I32Eqz);
            body.instruction(&Instruction::If(BlockType::Empty));
            emit_agent_retry_delay(
                body,
                indices,
                max_retries,
                retry_delay_ms,
                rate_limit_budget_ms,
            );
            emit_agent_retry_sleep(
                body,
                indices,
                static_data,
                durable_checkpoint,
                route_ptr_local,
                route_len_local,
                DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL,
                DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL,
            );
            emit_agent_record_retry_attempt(
                body,
                indices,
                route_ptr_local,
                route_len_local,
                DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL,
                DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL,
            );
            body.instruction(&Instruction::End); // !HIT gate
        } else {
            emit_agent_retry_delay(
                body,
                indices,
                max_retries,
                retry_delay_ms,
                rate_limit_budget_ms,
            );
            emit_agent_retry_sleep(
                body,
                indices,
                static_data,
                durable_checkpoint,
                route_ptr_local,
                route_len_local,
                DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL,
                DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL,
            );
        }
        body.instruction(&Instruction::Br(2));
        body.instruction(&Instruction::End);
        emit_agent_invoke_error_body_from_info(
            body,
            indices,
            static_data,
            track_events,
            agent_id,
            step_id,
            output_ptr_local,
            output_len_local,
            DIRECT_AGENT_RETRY_ERROR_PTR_LOCAL,
            DIRECT_AGENT_RETRY_ERROR_LEN_LOCAL,
            source_ptr_local,
            source_len_local,
            steps_ptr_local,
            steps_len_local,
            error_plan,
            route_ptr_local,
            route_len_local,
            variables,
            data_ptr_local,
            data_len_local,
            workflow_log_kind,
            workflow_error_kind,
            failure_target.map(|target| target.nested(3)),
            handled_target.map(|target| target.nested(3)),
        );
        body.instruction(&Instruction::End);
        load_agent_retptr_list(body, output_ptr_local, output_len_local);
        body.instruction(&Instruction::Br(1));
        body.instruction(&Instruction::End);
        body.instruction(&Instruction::End);
    } else {
        if let Some(slot_ptr_local) = memo_slot_ptr_local {
            // Parallel-split memoized invoke: a filled slot (state != 0) holds
            // the canonical `result<list<u8>, error-info>` the launch pass's
            // async-lowered call wrote through the slot retptr — copy it to
            // the retptr scratch so every downstream consumer (tag check,
            // error envelope, output materialization) reads the usual layout.
            // An EMPTY slot (launch skipped or failed) falls back to the
            // synchronous invoke, reproducing sequential semantics exactly.
            body.instruction(&Instruction::LocalGet(slot_ptr_local));
            body.instruction(&Instruction::I32Load(wasm_encoder::MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            body.instruction(&Instruction::If(BlockType::Empty));
            body.instruction(&Instruction::I32Const(0)); // dst: retptr scratch
            body.instruction(&Instruction::LocalGet(slot_ptr_local));
            body.instruction(&Instruction::I32Const(
                super::DIRECT_PSPLIT_SLOT_RESULT_OFFSET,
            ));
            body.instruction(&Instruction::I32Add);
            body.instruction(&Instruction::I32Const(super::DIRECT_PSPLIT_SLOT_RESULT_LEN));
            body.instruction(&Instruction::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            });
            // Consume-once (symmetry with the retry path; a single-attempt
            // step never re-reads it, but the slot must not look filled).
            body.instruction(&Instruction::LocalGet(slot_ptr_local));
            body.instruction(&Instruction::I32Const(0));
            body.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
                offset: 0,
                align: 2,
                memory_index: 0,
            }));
            body.instruction(&Instruction::Else);
            emit_agent_invoke(
                body,
                indices,
                invoke,
                capability_id,
                static_data,
                agent_id,
                output_ptr_local,
                output_len_local,
                source_ptr_local,
                source_len_local,
            );
            body.instruction(&Instruction::End);
        } else {
            emit_agent_invoke(
                body,
                indices,
                invoke,
                capability_id,
                static_data,
                agent_id,
                output_ptr_local,
                output_len_local,
                source_ptr_local,
                source_len_local,
            );
        }
        emit_agent_invoke_error_branch(
            body,
            indices,
            static_data,
            track_events,
            agent_id,
            step_id,
            output_ptr_local,
            output_len_local,
            source_ptr_local,
            source_len_local,
            steps_ptr_local,
            steps_len_local,
            error_plan,
            route_ptr_local,
            route_len_local,
            variables,
            data_ptr_local,
            data_len_local,
            workflow_log_kind,
            workflow_error_kind,
            failure_target,
            handled_target,
        );
        load_agent_retptr_list(body, output_ptr_local, output_len_local);
    }

    if durable_checkpoint {
        emit_checkpoint_save(
            body,
            indices,
            route_ptr_local,
            route_len_local,
            output_ptr_local,
            output_len_local,
        );
        body.instruction(&Instruction::End);
    }

    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(source_ptr_local));
    body.instruction(&Instruction::LocalGet(source_len_local));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::LocalGet(output_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(output_fn));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        route_ptr_local,
        route_len_local,
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

    // When this Agent ran through a parallel-window slot (the memoized assemble
    // path), carry the slot's real launch/settle wall-clock interval on the
    // debug-end event so the timeline/replay show true sibling overlap. A slot
    // with no stamp (0/0 — sequential, durable HIT, or a non-scheduler path)
    // records absent and falls back to assemble timing. Every non-parallel Agent
    // takes the plain end event.
    match memo_slot_ptr_local {
        Some(slot) => emit_step_debug_end_timed(
            body,
            indices,
            static_data,
            track_events,
            step_id,
            source_ptr_local,
            source_len_local,
            output_ptr_local,
            output_len_local,
            slot,
        ),
        None => emit_step_debug_event(
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
        ),
    }

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

#[allow(clippy::too_many_arguments)]
fn emit_agent_input_validation(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    track_events: bool,
    agent_id: u32,
    step_id: &str,
    input_ptr_local: u32,
    input_len_local: u32,
    source_ptr_local: u32,
    source_len_local: u32,
    steps_ptr_local: u32,
    steps_len_local: u32,
    error_plan: Option<&DirectErrorRoutePlan>,
    route_ptr_local: u32,
    route_len_local: u32,
    variables: DirectVariables<'_>,
    data_ptr_local: u32,
    data_len_local: u32,
    workflow_log_kind: &DirectDataSegment,
    workflow_error_kind: &DirectDataSegment,
    failure_target: Option<DirectFailureTarget>,
    handled_target: Option<DirectHandledTarget>,
) {
    body.instruction(&Instruction::I32Const(agent_id as i32));
    body.instruction(&Instruction::LocalGet(input_ptr_local));
    body.instruction(&Instruction::LocalGet(input_len_local));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.stdlib_agent_validate_input));
    emit_retptr_error_or_return(
        body,
        indices,
        failure_target,
        route_ptr_local,
        route_len_local,
    );
    load_retptr_list(body, route_ptr_local, route_len_local);

    body.instruction(&Instruction::LocalGet(route_len_local));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Ne);
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_agent_debug_error(
        body,
        indices,
        static_data,
        track_events,
        agent_id,
        source_ptr_local,
        source_len_local,
        route_ptr_local,
        route_len_local,
        input_ptr_local,
        input_len_local,
    );
    body.instruction(&Instruction::LocalGet(route_ptr_local));
    body.instruction(&Instruction::LocalSet(input_ptr_local));
    body.instruction(&Instruction::LocalGet(route_len_local));
    body.instruction(&Instruction::LocalSet(input_len_local));
    emit_agent_error_route_or_fail(
        body,
        indices,
        static_data,
        track_events,
        variables,
        step_id,
        input_ptr_local,
        input_len_local,
        steps_ptr_local,
        steps_len_local,
        source_ptr_local,
        source_len_local,
        input_ptr_local,
        input_len_local,
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
}
