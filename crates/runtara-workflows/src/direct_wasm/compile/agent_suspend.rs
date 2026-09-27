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
//!    continuation and attaches instance waits, then the workflow returns the
//!    unchanged lifecycle `suspended(at(min(earliest at-wake, step deadline)))`.
//!    A relaunch replays to this site, re-enters with the saved continuation and
//!    invokes again.
//! 3. `completed(bytes)`: the payload is moved to where a `capabilities` result
//!    keeps its list, so the ordinary output, checkpoint and error paths run
//!    unchanged; `scope.exit(false)`. An error arm leaves via `scope.exit(true)`.
//! 4. After the step's result checkpoint, `scope.release(checkpoint-key)`.
//!
//! Tracer scope (spike S0.2): one top-level, durable, timed, non-retrying
//! Agent step under the invoke ABI. [`check_sites`] refuses every other shape
//! instead of miscompiling it, together with the operation-scoped backstops
//! every control or suspending site needs.

use std::collections::BTreeSet;

use runtara_agent_suspension::layout;
use wasm_encoder::{BlockType, Function as WasmFunction, Instruction, MemArg};

use super::abi::{
    emit_suspend_at_return, load_retptr_tag, push_retptr_arg, push_retptr_i32_load,
    push_retptr_u8_load, return_if_retptr_error,
};
use super::{
    DIRECT_AGENT_ATTEMPT_ENV_LEN_LOCAL, DIRECT_AGENT_ATTEMPT_ENV_PTR_LOCAL,
    DIRECT_AGENT_ATTEMPT_KEY_LEN_LOCAL, DIRECT_AGENT_ATTEMPT_KEY_PTR_LOCAL,
    DIRECT_AGENT_RESULT_OK_LEN_OFFSET, DIRECT_AGENT_RESULT_OK_PTR_OFFSET,
    DIRECT_RETRY_PARK_DEADLINE_MS_LOCAL, DIRECT_RETRY_PARK_STATE_LEN_LOCAL,
    DIRECT_RETRY_PARK_STATE_PTR_LOCAL, DIRECT_RUN_RETPTR_OFFSET, DirectCompileError,
    DirectCoreFunctionIndices, DirectWorkflowManifest,
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

/// The first (and, in the tracer, only) attempt of an operation.
const FIRST_ATTEMPT: i32 = 1;

// Scratch locals of the suspended path. It runs only at a non-retrying site,
// where the per-attempt retry and retry-park locals are otherwise unused.
const WAKES_PTR: u32 = DIRECT_AGENT_ATTEMPT_ENV_PTR_LOCAL;
const WAKES_LEN: u32 = DIRECT_AGENT_ATTEMPT_ENV_LEN_LOCAL;
const STATE_PTR: u32 = DIRECT_RETRY_PARK_STATE_PTR_LOCAL;
const STATE_LEN: u32 = DIRECT_RETRY_PARK_STATE_LEN_LOCAL;
const CURSOR: u32 = DIRECT_AGENT_ATTEMPT_KEY_PTR_LOCAL;
const END: u32 = DIRECT_AGENT_ATTEMPT_KEY_LEN_LOCAL;
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
/// - a suspending site: non-durable, untimed, or an onError handler;
/// - until the lowering widens past the S0.2 tracer: a suspending site nested
///   in a loop or `onWait`, inside an embedded workflow, or retrying.
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
    // Embed call sites reached as AiAgent tools, then every embed below them.
    let mut tool_embeds = BTreeSet::new();
    for graph in std::iter::once(&manifest.graph)
        .chain(manifest.child_workflows.iter().map(|child| &child.graph))
    {
        collect_ai_targets(graph, &mut tool_embeds);
    }
    loop {
        let before = tool_embeds.len();
        for child in &manifest.child_workflows {
            if tool_embeds.contains(&child.step_id) {
                collect_embed_steps(&child.graph, &mut tool_embeds);
            }
        }
        if tool_embeds.len() == before {
            break;
        }
    }
    check_graph(&manifest.graph, SiteDepth::TopLevel, false, &target)?;
    for child in &manifest.child_workflows {
        let tool = tool_embeds.contains(&child.step_id);
        check_graph(&child.graph, SiteDepth::Embedded, tool, &target)?;
    }
    Ok(())
}

/// Where the graph holding a site sits.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SiteDepth {
    TopLevel,
    Nested,
    Embedded,
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

fn check_graph(
    graph: &DirectGraphManifest,
    depth: SiteDepth,
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
        // Limits of the S0.2 tracer lowering, lifted when it widens.
        match depth {
            SiteDepth::TopLevel => {}
            SiteDepth::Nested => {
                return refuse(
                    step,
                    kind,
                    "is supported only at the top level of a workflow",
                );
            }
            SiteDepth::Embedded => {
                return refuse(step, kind, "cannot run inside an embedded workflow");
            }
        }
        if agent.max_retries.unwrap_or(1) != 0 {
            return refuse(step, kind, "must set maxRetries to 0");
        }
    }
    let nested_depth = match depth {
        SiteDepth::Embedded => SiteDepth::Embedded,
        SiteDepth::TopLevel | SiteDepth::Nested => SiteDepth::Nested,
    };
    for step in &graph.steps {
        for nested in &step.nested_graphs {
            check_graph(&nested.graph, nested_depth, in_tool_workflow, target)?;
        }
    }
    Ok(())
}

/// `scope.enter(checkpoint-key, 1, true)`; a host failure fails the workflow.
pub(super) fn emit_enter(
    body: &mut WasmFunction,
    indices: &DirectCoreFunctionIndices,
    key_ptr_local: u32,
    key_len_local: u32,
) {
    body.instruction(&Instruction::LocalGet(key_ptr_local));
    body.instruction(&Instruction::LocalGet(key_len_local));
    body.instruction(&Instruction::I32Const(FIRST_ATTEMPT));
    body.instruction(&Instruction::I32Const(1)); // load the continuation
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.operation_scope().enter));
    return_if_retptr_error(body, indices);
}

/// Consume a `suspendable.invoke` result left in the retptr area.
///
/// A suspension parks the workflow and returns from the entry function. A
/// completed outcome is rewritten in place to the `capabilities` result
/// layout; an error arm already has it. Either way the operation is exited
/// and the caller's ordinary error/output handling continues.
pub(super) fn emit_after_invoke(body: &mut WasmFunction, indices: &DirectCoreFunctionIndices) {
    let scope = indices.operation_scope();
    load_retptr_tag(body);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    push_retptr_u8_load(body, OUTCOME_DISCRIMINANT_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_suspend(body, indices);
    body.instruction(&Instruction::End);
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
    body.instruction(&Instruction::Else);
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::Call(scope.exit));
    body.instruction(&Instruction::End);
}

/// Persist the suspension and park at the earliest `at` wake, bounded by the
/// step deadline (`agent_deadline::DEADLINE`), which every suspending site has.
fn emit_suspend(body: &mut WasmFunction, indices: &DirectCoreFunctionIndices) {
    for (offset, local) in [
        (SUSPENDED_WAKES_PTR_OFFSET, WAKES_PTR),
        (SUSPENDED_WAKES_LEN_OFFSET, WAKES_LEN),
        (SUSPENDED_STATE_PTR_OFFSET, STATE_PTR),
        (SUSPENDED_STATE_LEN_OFFSET, STATE_LEN),
    ] {
        push_retptr_i32_load(body, offset);
        body.instruction(&Instruction::LocalSet(local));
    }
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
    return_if_retptr_error(body, indices);
    emit_suspend_at_return(body, indices, PARK_AT);
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
