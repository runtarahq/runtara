// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Lowering of a suspending Agent call site (typed agent suspension).
//!
//! A capability the catalog declares `suspends` is invoked through its
//! agent's `suspendable` interface instead of `capabilities`, inside the
//! compiler-emitted `runtara:workflow-operation/scope`:
//!
//! 1. `scope.enter(checkpoint-key, attempt, load)` names the operation. The
//!    host derives `op_hash = sha256(checkpoint-key)` and hands the saved
//!    continuation to the capability itself (`runtara:agent-suspension/context`
//!    or the control executor's argument). `suspendable.invoke` never carries
//!    one, so this module passes none.
//! 2. `suspended { wakes, state }`: `scope.suspend(state, wakes)` persists the
//!    continuation for the entered attempt and attaches the instance waits the
//!    operation registered, then the workflow returns the unchanged lifecycle
//!    `suspended(at(min(earliest at-wake, step deadline)))`, clamped by any
//!    enclosing loop deadline. Nothing is checkpointed for the attempt (no
//!    `::attempt::` key): a suspension is not a failure. A relaunch replays to
//!    this site, re-enters the same attempt with the saved continuation and
//!    invokes again. A suspension within one second of the step deadline would
//!    only wake to time out, so it fails the step with `AGENT_TIMEOUT` instead;
//!    one the host refuses (caps, a wait the operation did not register) fails
//!    it with `AGENT_INVALID_SUSPENSION`. Either failure leaves the operation
//!    through `scope.exit(true)`.
//! 3. `completed(bytes)`: the payload is moved to where a `capabilities` result
//!    keeps its list, so the ordinary output, checkpoint, retry and error paths
//!    run unchanged; `scope.exit(false)`. An error arm leaves via
//!    `scope.exit(true)`, which closes the operation's wait and discards its
//!    continuation, so a retry (the next attempt) or a replay starts afresh.
//! 4. After the step's result checkpoint, `scope.release(checkpoint-key)`.
//!
//! Sites may sit wherever the slice-4 context matrix allows: the top level,
//! branch arms (a branch group holding one runs sequentially), sequential Split
//! and While bodies (a parallel Split runs its body sequentially), embedded
//! workflows, and retrying steps, where each attempt is its own continuation.
//! [`check_sites`] refuses every other shape instead of miscompiling it,
//! together with the operation-scoped backstops every control or suspending
//! site needs.

use std::collections::BTreeSet;

use runtara_agent_suspension::layout;
use wasm_encoder::{BlockType, Function as WasmFunction, Instruction, MemArg};

use super::abi::{
    emit_suspend_at_return, load_retptr_tag, push_retptr_arg, push_retptr_i32_load,
    push_retptr_i64_load, push_retptr_u8_load, return_if_retptr_error,
};
use super::{
    DIRECT_AGENT_ATTEMPT_ENV_LEN_LOCAL, DIRECT_AGENT_ATTEMPT_ENV_PTR_LOCAL,
    DIRECT_AGENT_RESULT_OK_LEN_OFFSET, DIRECT_AGENT_RESULT_OK_PTR_OFFSET, DIRECT_RET_U64_OK_OFFSET,
    DIRECT_RETRY_PARK_DEADLINE_MS_LOCAL, DIRECT_RETRY_PARK_STATE_LEN_LOCAL,
    DIRECT_RETRY_PARK_STATE_PTR_LOCAL, DIRECT_RUN_RETPTR_OFFSET, DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL,
    DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL, DirectCompileError, DirectCoreFunctionIndices,
    DirectCoreStaticData, DirectWorkflowManifest,
};
use crate::direct_wasm::component::{RuntimeBinding, WorkflowAbi};
use crate::direct_wasm::manifest::{DirectAgentManifest, DirectEdgeManifest, DirectGraphManifest};

// Canonical layout of `result<outcome, error-info>` in the retptr area, where
// `outcome = variant { completed(list<u8>), suspended(suspension) }`,
// `suspension = record { wakes: list<wake>, state: list<u8> }` and
// `wake = variant { at(u64), instances(string) }`. The error arm is the same
// `error-info` at +8 as `capabilities.invoke`'s. Every offset comes from
// `runtara_agent_suspension::layout`, which `runtara-workflow-wit` pins against
// the WIT's `SizeAlign`; `agent_suspend_tests` re-derives them here.

/// Discriminant of `outcome` (0 = completed, 1 = suspended).
pub(super) const OUTCOME_DISCRIMINANT_OFFSET: u64 = layout::INVOKE_RESULT_PAYLOAD_OFFSET as u64;
const OUTCOME_PAYLOAD: u64 = OUTCOME_DISCRIMINANT_OFFSET + layout::OUTCOME_PAYLOAD_OFFSET as u64;
/// `completed(list<u8>)` pointer and length.
pub(super) const COMPLETED_PTR_OFFSET: u64 = OUTCOME_PAYLOAD;
pub(super) const COMPLETED_LEN_OFFSET: u64 = COMPLETED_PTR_OFFSET + 4;
/// `suspended(suspension)`: `wakes` then `state`, each pointer and length.
pub(super) const SUSPENDED_WAKES_PTR_OFFSET: u64 =
    OUTCOME_PAYLOAD + layout::SUSPENSION_WAKES_OFFSET as u64;
pub(super) const SUSPENDED_WAKES_LEN_OFFSET: u64 = SUSPENDED_WAKES_PTR_OFFSET + 4;
pub(super) const SUSPENDED_STATE_PTR_OFFSET: u64 =
    OUTCOME_PAYLOAD + layout::SUSPENSION_STATE_OFFSET as u64;
pub(super) const SUSPENDED_STATE_LEN_OFFSET: u64 = SUSPENDED_STATE_PTR_OFFSET + 4;
/// One `wake` element: discriminant at +0 (0 = at), `at` value at +8.
pub(super) const WAKE_SIZE: i32 = layout::WAKE_SIZE as i32;
pub(super) const WAKE_AT_VALUE_OFFSET: u64 = layout::WAKE_PAYLOAD_OFFSET as u64;

/// The attempt of an operation at a site without retries.
const FIRST_ATTEMPT: i32 = 1;

// Scratch locals of the suspended path. It either returns from the entry
// function (the park) or falls through to the step's error path, so it may
// only borrow locals that path sets before reading: the per-attempt envelope
// and retry-park locals are rewritten on every failure, and the Wait/Delay
// signal-id locals belong to other step types. The per-attempt checkpoint key
// of a retrying site stays intact for its failure checkpoint.
const WAKES_PTR: u32 = DIRECT_AGENT_ATTEMPT_ENV_PTR_LOCAL;
const WAKES_LEN: u32 = DIRECT_AGENT_ATTEMPT_ENV_LEN_LOCAL;
const STATE_PTR: u32 = DIRECT_RETRY_PARK_STATE_PTR_LOCAL;
const STATE_LEN: u32 = DIRECT_RETRY_PARK_STATE_LEN_LOCAL;
const CURSOR: u32 = DIRECT_WAIT_SIGNAL_ID_PTR_LOCAL;
const END: u32 = DIRECT_WAIT_SIGNAL_ID_LEN_LOCAL;
const PARK_AT: u32 = DIRECT_RETRY_PARK_DEADLINE_MS_LOCAL;

/// Compile-time backstop for operation-scoped (suspending or control) Agent
/// call sites. Validation reports the same rules with stable codes (E028,
/// E029, E131, E132); this refuses whatever reaches the compiler anyway, plus
/// shapes only the compiler can see:
///
/// - any operation-scoped site: an AiAgent tool, memory provider or synthetic
///   AiAgent call, a workflow embedded as an AiAgent tool, the `CliRunHttp`
///   ABI (it blocks instead of parking), the `AgentCapabilities` ABI (a
///   published workflow-agent), the composed runtime binding, an omitted
///   runtime, scoped isolation, and a control call compiled without the agent
///   catalog (it could not be classified);
/// - any operation-scoped site in a `WaitForSignal.onWait` graph, directly or
///   through an embedded workflow;
/// - a suspending site: non-durable, untimed, or an onError handler;
/// - a WaitForInstances step wherever a suspending site is refused, except
///   that it has no step timeout: an AiAgent tool or memory target, a tool
///   workflow, an onWait graph, a target other than the lifecycle invoke ABI
///   with the host-imported runtime, a non-durable graph, or an onError
///   handler.
pub(super) fn check_sites(
    manifest: &DirectWorkflowManifest,
    abi: WorkflowAbi,
    omit_runtime: bool,
    runtime_binding: RuntimeBinding,
    scoped_agents: &BTreeSet<String>,
    has_catalog: bool,
) -> Result<(), DirectCompileError> {
    let target = SiteTarget {
        abi,
        omit_runtime,
        runtime_binding,
        scoped_agents,
    };
    if !has_catalog {
        let control = std::iter::once(&manifest.graph)
            .chain(manifest.child_workflows.iter().map(|child| &child.graph))
            .find_map(|graph| find_agent(graph, &|agent| is_control_agent(&agent.agent_id)));
        if let Some(step) = control {
            return refuse(
                step,
                "a control-agent",
                "cannot be classified without the agent catalog; compile with the catalog",
            );
        }
    }
    // Embed call sites reached as AiAgent tools, and embeds inside a
    // `WaitForSignal.onWait` graph, then every embed below them.
    let mut tool_embeds = BTreeSet::new();
    let mut on_wait_embeds = BTreeSet::new();
    for graph in std::iter::once(&manifest.graph)
        .chain(manifest.child_workflows.iter().map(|child| &child.graph))
    {
        collect_ai_targets(graph, &mut tool_embeds);
        collect_on_wait_embeds(graph, false, &mut on_wait_embeds);
    }
    for embeds in [&mut tool_embeds, &mut on_wait_embeds] {
        loop {
            let before = embeds.len();
            for child in &manifest.child_workflows {
                if embeds.contains(&child.step_id) {
                    collect_embed_steps(&child.graph, embeds);
                }
            }
            if embeds.len() == before {
                break;
            }
        }
    }
    check_graph(&manifest.graph, false, false, &target)?;
    for child in &manifest.child_workflows {
        check_graph(
            &child.graph,
            on_wait_embeds.contains(&child.step_id),
            tool_embeds.contains(&child.step_id),
            &target,
        )?;
    }
    Ok(())
}

/// The compile target every operation-scoped site must support.
struct SiteTarget<'a> {
    abi: WorkflowAbi,
    omit_runtime: bool,
    runtime_binding: RuntimeBinding,
    scoped_agents: &'a BTreeSet<String>,
}

fn refuse(step: &str, kind: &str, why: &str) -> Result<(), DirectCompileError> {
    Err(DirectCompileError::Component(format!(
        "Agent step `{step}` calls {kind} capability, which {why}"
    )))
}

fn is_control_agent(agent_id: &str) -> bool {
    runtara_dsl::agent_meta::canonical_agent_id(agent_id)
        == runtara_dsl::agent_meta::CONTROL_AGENT_ID
}

fn find_agent<'a>(
    graph: &'a DirectGraphManifest,
    matches: &dyn Fn(&DirectAgentManifest) -> bool,
) -> Option<&'a str> {
    graph
        .agents
        .iter()
        .find(|agent| matches(agent))
        .map(|agent| agent.step_id.as_str())
        .or_else(|| {
            graph.steps.iter().find_map(|step| {
                step.nested_graphs
                    .iter()
                    .find_map(|nested| find_agent(&nested.graph, matches))
            })
        })
}

/// An AiAgent edge other than its continuation or onError: a tool, MCP or
/// memory edge.
fn is_ai_edge(graph: &DirectGraphManifest, edge: &DirectEdgeManifest) -> bool {
    !matches!(edge.label.as_deref(), None | Some("next") | Some("onError"))
        && graph
            .steps
            .iter()
            .any(|step| step.id == edge.from_step && step.step_type == "AiAgent")
}

/// Targets of AiAgent tool and memory edges in `graph` and its nested graphs.
fn collect_ai_targets(graph: &DirectGraphManifest, targets: &mut BTreeSet<String>) {
    for edge in graph.edges.iter().filter(|edge| is_ai_edge(graph, edge)) {
        targets.insert(edge.to_step.clone());
    }
    for step in &graph.steps {
        for nested in &step.nested_graphs {
            collect_ai_targets(&nested.graph, targets);
        }
    }
}

fn collect_embed_steps(graph: &DirectGraphManifest, embeds: &mut BTreeSet<String>) {
    for step in &graph.steps {
        if step.step_type == "EmbedWorkflow" {
            embeds.insert(step.id.clone());
        }
        for nested in &step.nested_graphs {
            collect_embed_steps(&nested.graph, embeds);
        }
    }
}

/// Embed steps inside `WaitForSignal.onWait` graphs of `graph`.
fn collect_on_wait_embeds(
    graph: &DirectGraphManifest,
    in_on_wait: bool,
    embeds: &mut BTreeSet<String>,
) {
    for step in &graph.steps {
        if in_on_wait && step.step_type == "EmbedWorkflow" {
            embeds.insert(step.id.clone());
        }
        for nested in &step.nested_graphs {
            let on_wait = in_on_wait || nested.role == ON_WAIT_ROLE;
            collect_on_wait_embeds(&nested.graph, on_wait, embeds);
        }
    }
}

/// Role of a `WaitForSignal.onWait` nested graph.
const ON_WAIT_ROLE: &str = "waitForSignal.onWait";

fn check_graph(
    graph: &DirectGraphManifest,
    in_on_wait: bool,
    in_tool_workflow: bool,
    target: &SiteTarget<'_>,
) -> Result<(), DirectCompileError> {
    for agent in graph.agents.iter().filter(|agent| agent.operation_scoped) {
        let step = agent.step_id.as_str();
        let kind = if agent.suspends {
            "a suspending"
        } else {
            "a control-agent"
        };
        if agent.step_type != "Agent" || agent.purpose != "agent.config" {
            return refuse(step, kind, "cannot back an AiAgent");
        }
        let callers = graph.edges.iter().filter(|edge| edge.to_step == step);
        if callers.clone().any(|edge| is_ai_edge(graph, edge)) {
            return refuse(step, kind, "cannot be an AiAgent tool or memory provider");
        }
        if in_tool_workflow {
            return refuse(
                step,
                kind,
                "cannot run in a workflow embedded as an AiAgent tool",
            );
        }
        if in_on_wait {
            return refuse(step, kind, "cannot run in a WaitForSignal onWait graph");
        }
        match target.abi {
            WorkflowAbi::InvokeHostImports => {}
            WorkflowAbi::CliRunHttp => {
                return refuse(
                    step,
                    kind,
                    "needs the lifecycle invoke ABI; the CliRunHttp ABI blocks instead of parking",
                );
            }
            WorkflowAbi::AgentCapabilities => {
                return refuse(step, kind, "cannot be published as a workflow-agent");
            }
        }
        if target.runtime_binding != RuntimeBinding::HostImport {
            return refuse(
                step,
                kind,
                "cannot compile under the composed runtime binding; use the host-imported runtime",
            );
        }
        if target.omit_runtime {
            return refuse(step, kind, "needs the host-imported workflow runtime");
        }
        if target.scoped_agents.contains(&agent.agent_id) {
            return refuse(step, kind, "cannot run under scoped isolation");
        }
        if !agent.suspends {
            continue;
        }
        if !agent.durable {
            return refuse(step, kind, "must be durable");
        }
        if agent.timeout.unwrap_or(0) == 0 {
            return refuse(step, kind, "needs a timeout");
        }
        if callers
            .clone()
            .any(|edge| edge.label.as_deref() == Some("onError"))
        {
            return refuse(step, kind, "cannot run in an onError handler");
        }
    }
    for step in graph
        .steps
        .iter()
        .filter(|step| step.step_type == "WaitForInstances")
    {
        check_wait_for_instances(graph, &step.id, in_on_wait, in_tool_workflow, target)?;
    }
    for step in &graph.steps {
        for nested in &step.nested_graphs {
            let on_wait = in_on_wait || nested.role == ON_WAIT_ROLE;
            check_graph(&nested.graph, on_wait, in_tool_workflow, target)?;
        }
    }
    Ok(())
}

fn refuse_wait(step: &str, why: &str) -> Result<(), DirectCompileError> {
    Err(DirectCompileError::Component(format!(
        "WaitForInstances step `{step}` {why}"
    )))
}

/// The WaitForInstances backstop: the placements validation rejects (E028,
/// E131) and the targets that cannot park on an instance wait.
fn check_wait_for_instances(
    graph: &DirectGraphManifest,
    step: &str,
    in_on_wait: bool,
    in_tool_workflow: bool,
    target: &SiteTarget<'_>,
) -> Result<(), DirectCompileError> {
    let callers = graph.edges.iter().filter(|edge| edge.to_step == step);
    if callers.clone().any(|edge| is_ai_edge(graph, edge)) {
        return refuse_wait(step, "cannot be an AiAgent tool or memory provider");
    }
    if in_tool_workflow {
        return refuse_wait(step, "cannot run in a workflow embedded as an AiAgent tool");
    }
    if in_on_wait {
        return refuse_wait(step, "cannot run in a WaitForSignal onWait graph");
    }
    match target.abi {
        WorkflowAbi::InvokeHostImports => {}
        WorkflowAbi::CliRunHttp => {
            return refuse_wait(
                step,
                "needs the lifecycle invoke ABI; the CliRunHttp ABI blocks instead of parking",
            );
        }
        WorkflowAbi::AgentCapabilities => {
            return refuse_wait(step, "cannot be published as a workflow-agent");
        }
    }
    if target.runtime_binding != RuntimeBinding::HostImport {
        return refuse_wait(
            step,
            "cannot compile under the composed runtime binding; use the host-imported runtime",
        );
    }
    if target.omit_runtime {
        return refuse_wait(step, "needs the host-imported workflow runtime");
    }
    if !graph.durable {
        return refuse_wait(step, "must be durable");
    }
    if callers
        .clone()
        .any(|edge| edge.label.as_deref() == Some("onError"))
    {
        return refuse_wait(step, "cannot run in an onError handler");
    }
    Ok(())
}

/// `scope.enter(checkpoint-key, attempt, load)` before the site's invoke,
/// which runs its deadline check first. `attempt` is the retry-attempt local
/// of a retrying site (attempt 1 otherwise); only a suspending site loads its
/// continuation.
///
/// A refused enter becomes the step error `AGENT_OPERATION_SCOPE` in the
/// retptr area, so the ordinary error path (onError, retry classification)
/// runs without an invoke. This opens an `if … else`: the caller emits the
/// invoke into the `else` arm and closes it with [`emit_entered_end`], then
/// leaves with [`emit_after_invoke`] or [`emit_exit`].
pub(super) fn emit_enter(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
    key: (u32, u32),
    attempt_local: Option<u32>,
    load: bool,
) {
    body.instruction(&Instruction::LocalGet(key.0));
    body.instruction(&Instruction::LocalGet(key.1));
    match attempt_local {
        Some(local) => body.instruction(&Instruction::LocalGet(local)),
        None => body.instruction(&Instruction::I32Const(FIRST_ATTEMPT)),
    };
    body.instruction(&Instruction::I32Const(i32::from(load)));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.operation_scope().enter));
    load_retptr_tag(body);
    body.instruction(&Instruction::If(BlockType::Empty));
    let segment = static_data
        .operation_scope_error
        .as_ref()
        .expect("an operation-scoped site lays out its scope error");
    super::agent_deadline::error_info(
        body,
        segment.offset,
        crate::direct_wasm::static_data::AGENT_OPERATION_SCOPE_FIELDS,
    );
    body.instruction(&Instruction::Else);
}

/// Close the `if … else` [`emit_enter`] opened, after the invoke.
pub(super) fn emit_entered_end(body: &mut WasmFunction) {
    body.instruction(&Instruction::End);
}

/// `scope.exit(false)` after a non-suspending scoped site's invoke, on every
/// outcome: it has no continuation to discard, and a refused enter left no
/// operation to leave (the host ignores it).
pub(super) fn emit_exit(body: &mut WasmFunction, indices: &DirectCoreFunctionIndices) {
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::Call(indices.operation_scope().exit));
}

/// Consume a `suspendable.invoke` result left in the retptr area.
///
/// A suspension parks the workflow and returns from the entry function,
/// unless it is refused or too close to the step deadline: then the retptr
/// area holds the step error (`AGENT_INVALID_SUSPENSION`, `AGENT_TIMEOUT`).
/// A completed outcome is rewritten in place to the `capabilities` result
/// layout; an error arm already has it. Either way the operation is exited
/// and the caller's ordinary error/output handling continues.
pub(super) fn emit_after_invoke(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
) {
    let scope = indices.operation_scope();
    load_retptr_tag(body);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    push_retptr_u8_load(body, OUTCOME_DISCRIMINANT_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    // Suspended: park, or leave the step error in the retptr area.
    emit_suspend(body, indices, static_data);
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::Call(scope.exit));
    body.instruction(&Instruction::Else);
    // completed(list<u8>) at +12/+16 → the `ok(list<u8>)` slots at +8/+12.
    // Each load happens before the store that could overlap it.
    for (from, to) in [
        (COMPLETED_PTR_OFFSET, DIRECT_AGENT_RESULT_OK_PTR_OFFSET),
        (COMPLETED_LEN_OFFSET, DIRECT_AGENT_RESULT_OK_LEN_OFFSET),
    ] {
        body.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
        push_retptr_i32_load(body, from);
        body.instruction(&Instruction::I32Store(MemArg {
            offset: to,
            align: 2,
            memory_index: 0,
        }));
    }
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::Call(scope.exit));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::Call(scope.exit));
    body.instruction(&Instruction::End);
}

/// A suspension this close to the step deadline fails with `AGENT_TIMEOUT`
/// instead of parking: its wake could only time the step out.
pub(super) const DEADLINE_MARGIN_MS: i64 = 1_000;

/// Persist the suspension and park at the earliest `at` wake, bounded by the
/// step deadline (`agent_deadline::DEADLINE`), which every suspending site has.
/// Falls through only with a step error in the retptr area.
fn emit_suspend(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    static_data: &DirectCoreStaticData,
) {
    // Stash the suspension first: every host call below reuses the retptr.
    for (offset, local) in [
        (SUSPENDED_WAKES_PTR_OFFSET, WAKES_PTR),
        (SUSPENDED_WAKES_LEN_OFFSET, WAKES_LEN),
        (SUSPENDED_STATE_PTR_OFFSET, STATE_PTR),
        (SUSPENDED_STATE_LEN_OFFSET, STATE_LEN),
    ] {
        push_retptr_i32_load(body, offset);
        body.instruction(&Instruction::LocalSet(local));
    }
    // now + margin >= DEADLINE: the step times out instead of parking.
    push_retptr_arg(body);
    super::abi::emit_call_wide_result(body, indices.runtime_now_ms);
    return_if_retptr_error(body, indices);
    push_retptr_i64_load(body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::I64Const(DEADLINE_MARGIN_MS));
    body.instruction(&Instruction::I64Add);
    body.instruction(&Instruction::LocalGet(super::agent_deadline::DEADLINE));
    body.instruction(&Instruction::I64GeU);
    body.instruction(&Instruction::If(BlockType::Empty));
    super::agent_deadline::error(body, static_data);
    body.instruction(&Instruction::Else);

    body.instruction(&Instruction::LocalGet(super::agent_deadline::DEADLINE));
    body.instruction(&Instruction::LocalSet(PARK_AT));
    // The lifted list lies in this memory, so `len * 16` cannot overflow it.
    body.instruction(&Instruction::LocalGet(WAKES_PTR));
    body.instruction(&Instruction::LocalSet(CURSOR));
    body.instruction(&Instruction::LocalGet(WAKES_PTR));
    body.instruction(&Instruction::LocalGet(WAKES_LEN));
    body.instruction(&Instruction::I32Const(WAKE_SIZE));
    body.instruction(&Instruction::I32Mul);
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(END));
    body.instruction(&Instruction::Block(BlockType::Empty));
    body.instruction(&Instruction::Loop(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(CURSOR));
    body.instruction(&Instruction::LocalGet(END));
    body.instruction(&Instruction::I32GeU);
    body.instruction(&Instruction::BrIf(1));
    // wake::at: keep the earliest.
    body.instruction(&Instruction::LocalGet(CURSOR));
    body.instruction(&Instruction::I32Load8U(MemArg {
        offset: 0,
        align: 0,
        memory_index: 0,
    }));
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    let at = MemArg {
        offset: WAKE_AT_VALUE_OFFSET,
        align: 3,
        memory_index: 0,
    };
    body.instruction(&Instruction::LocalGet(CURSOR));
    body.instruction(&Instruction::I64Load(at));
    body.instruction(&Instruction::LocalGet(PARK_AT));
    body.instruction(&Instruction::I64LtU);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(CURSOR));
    body.instruction(&Instruction::I64Load(at));
    body.instruction(&Instruction::LocalSet(PARK_AT));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalGet(CURSOR));
    body.instruction(&Instruction::I32Const(WAKE_SIZE));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(CURSOR));
    body.instruction(&Instruction::Br(0));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);

    body.instruction(&Instruction::LocalGet(STATE_PTR));
    body.instruction(&Instruction::LocalGet(STATE_LEN));
    body.instruction(&Instruction::LocalGet(WAKES_PTR));
    body.instruction(&Instruction::LocalGet(WAKES_LEN));
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.operation_scope().suspend));
    load_retptr_tag(body);
    body.instruction(&Instruction::If(BlockType::Empty));
    // The host refused the suspension: a step error, not a park.
    let segment = static_data
        .invalid_suspension_error
        .as_ref()
        .expect("a suspending site lays out its suspension error");
    super::agent_deadline::error_info(
        body,
        segment.offset,
        crate::direct_wasm::static_data::AGENT_INVALID_SUSPENSION_FIELDS,
    );
    body.instruction(&Instruction::Else);
    emit_suspend_at_return(body, indices, PARK_AT);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
}

/// `scope.release(checkpoint-key)` once the step result is checkpointed.
pub(super) fn emit_release(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    key_ptr_local: u32,
    key_len_local: u32,
) {
    body.instruction(&Instruction::LocalGet(key_ptr_local));
    body.instruction(&Instruction::LocalGet(key_len_local));
    body.instruction(&Instruction::Call(indices.operation_scope().release));
}
