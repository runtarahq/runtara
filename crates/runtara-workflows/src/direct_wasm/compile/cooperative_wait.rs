//! Guest-owned cooperative waits for sequential and parallel component calls.
//!
//! The Agent writes its result at 0. Polling uses 128..172, after that result
//! and before the wait event at 216; neither asynchronous completion can
//! overwrite the other. Handles stay in guest locals; there is no host task
//! interface. Nonempty runtime results still use canonical ABI allocation.
use wasm_encoder::{BlockType, Function, Instruction, MemArg};

use super::abi::{
    emit_entry_cancel_return, emit_entry_suspend_return, emit_fail_if_retptr_error_inplace,
    push_retptr_arg,
};
use super::{DIRECT_PSPLIT_EVENT_OFFSET, DirectCoreFunctionIndices};

const TARGET: u32 = 142;
const SET: u32 = 143;
const TIMER: u32 = 144;
const STATUS: u32 = 145;
// Parallel windows reuse the emitter's existing per-call slots. Separate
// bounds survive scheduler scratch-local reuse and sequential retry/fallback.
const WINDOW_END: u32 = 146;
const CURSOR: u32 = 147;
const HANDLE: u32 = 148;
const WINDOW_TIMER: u32 = 149;
const WINDOW_ACTIVE: u32 = 150;
const DEFER_BOUNDARY: u32 = 151;
const SIGNAL_PTR: u32 = 152;
const SIGNAL_LEN: u32 = 153;
const COMMAND_PTR: u32 = 154;
const COMMAND_LEN: u32 = 155;
const SIGNAL_PENDING: u32 = 156;
const WINDOW_BEGIN: u32 = 157;
// An optional owned deadline arrives as a canonical async call status. The
// enclosing scope creates its timer; this wait resolves it before returning.
pub(super) const DEADLINE_STATUS: u32 = 158;
pub(super) const TIMED_OUT: u32 = 159;
const POLL: i32 = 128;
const RETURNED: i32 = 2;
const START_CANCELLED: i32 = 3;
const CANCELLED: i32 = 4;
const POLL_INTERVAL_MS: i64 = 1_000;

// Shared core functions receive and return invocation-local state as Wasm
// values. No globals, heap frame, or host-owned tasks are needed.
// Scratch cursors/handles are deliberately excluded. STATUS is the packed
// input to Await; the final extra result describes entry control flow.
const STATE: [u32; 16] = [
    TARGET,
    SET,
    TIMER,
    STATUS,
    WINDOW_END,
    WINDOW_TIMER,
    WINDOW_ACTIVE,
    DEFER_BOUNDARY,
    SIGNAL_PTR,
    SIGNAL_LEN,
    COMMAND_PTR,
    COMMAND_LEN,
    SIGNAL_PENDING,
    WINDOW_BEGIN,
    super::DIRECT_PSPLIT_WS_LOCAL,
    DEADLINE_STATUS,
];
pub(super) const HELPER_PARAMS: usize = STATE.len();
pub(super) const HELPER_COUNT: usize = 6;

#[derive(Clone, Copy)]
pub(super) enum Helper {
    Poll,
    Boundary,
    Checkpoint,
    WindowCancel,
    Await,
    WindowWait,
}

impl Helper {
    pub(super) const ALL: [Self; HELPER_COUNT] = [
        Self::Poll,
        Self::Boundary,
        Self::Checkpoint,
        Self::WindowCancel,
        Self::Await,
        Self::WindowWait,
    ];

    pub(super) fn needs_runtime(self) -> bool {
        matches!(self, Self::Poll | Self::Boundary | Self::Checkpoint)
    }
}

fn helper_return(body: &mut Function, outcome: i32) {
    for local in STATE {
        body.instruction(&Instruction::LocalGet(local));
    }
    body.instruction(&Instruction::I32Const(outcome));
    body.instruction(&Instruction::Return);
}

/// Outcomes: 0 resumes, 1 propagates the error at zero, 2 suspends, 3 returns
/// from an invocation cancelled by its composing parent (without a root ack).
/// Outcome 4 selects the scope deadline after resolving this wait's handles.
/// Entry-ABI-specific terminal reporting remains in the entry function.
fn call_helper(body: &mut Function, indices: &DirectCoreFunctionIndices, helper: Helper) -> bool {
    let Some(index) = indices.cooperative_helpers[helper as usize] else {
        return false;
    };
    for local in STATE {
        body.instruction(&Instruction::LocalGet(local));
    }
    body.instruction(&Instruction::Call(index));
    body.instruction(&Instruction::LocalSet(CURSOR));
    for local in STATE.into_iter().rev() {
        body.instruction(&Instruction::LocalSet(local));
    }
    body.instruction(&Instruction::LocalGet(CURSOR));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_fail_if_retptr_error_inplace(body, indices);
    // An error outcome must never continue the workflow, even if a malformed
    // runtime response somehow omitted its error tag.
    body.instruction(&Instruction::Unreachable);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalGet(CURSOR));
    body.instruction(&Instruction::I32Const(2));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_entry_suspend_return(body, indices);
    body.instruction(&Instruction::End);
    if matches!(
        indices.abi,
        crate::direct_wasm::component::WorkflowAbi::AgentCapabilities
    ) {
        body.instruction(&Instruction::LocalGet(CURSOR));
        body.instruction(&Instruction::I32Const(3));
        body.instruction(&Instruction::I32Eq);
        body.instruction(&Instruction::If(BlockType::Empty));
        emit_entry_cancel_return(body);
        body.instruction(&Instruction::End);
    }
    if matches!(helper, Helper::Await | Helper::WindowWait) {
        body.instruction(&Instruction::LocalGet(CURSOR));
        body.instruction(&Instruction::I32Const(4));
        body.instruction(&Instruction::I32Eq);
        body.instruction(&Instruction::LocalSet(TIMED_OUT));
    }
    true
}

/// Consume the standard wait event. A composed workflow-agent owns only its
/// nested handles; the parent retains the root signal and chooses what follows.
fn handle_wait_event(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    if matches!(
        indices.abi,
        crate::direct_wasm::component::WorkflowAbi::AgentCapabilities
    ) {
        body.instruction(&Instruction::I32Const(6));
        body.instruction(&Instruction::I32Eq);
        body.instruction(&Instruction::If(BlockType::Empty));
        close_all(body, indices);
        if indices.cooperative_helper_body {
            helper_return(body, 3);
        } else {
            emit_entry_cancel_return(body);
        }
        body.instruction(&Instruction::End);
    } else {
        body.instruction(&Instruction::Drop);
    }
}

pub(super) fn helper_body(helper: Helper, indices: &DirectCoreFunctionIndices) -> Function {
    // Keep the emitter's canonical absolute local indices. Parameters occupy
    // its first 16 i32 slots and are copied to the state locals before use.
    let mut body = Function::new(super::core_module::drop_leading_locals(
        super::core_module::CANONICAL_LOCAL_GROUPS,
        HELPER_PARAMS as u32,
    ));
    for (param, local) in STATE.into_iter().enumerate() {
        body.instruction(&Instruction::LocalGet(param as u32));
        body.instruction(&Instruction::LocalSet(local));
    }
    let mut inline = indices.clone();
    inline.cooperative_helpers = [None; HELPER_COUNT];
    inline.cooperative_helper_body = true;
    match helper {
        Helper::Poll => emit_poll_before_call(&mut body, &inline),
        Helper::Boundary => emit_retained_boundary(&mut body, &inline),
        Helper::Checkpoint => emit_checkpoint_signal(&mut body, &inline),
        Helper::WindowCancel => cancel_window_deadline(&mut body, &inline),
        Helper::Await => {
            body.instruction(&Instruction::LocalGet(STATUS));
            emit_await_call(&mut body, &inline);
        }
        Helper::WindowWait => emit_window_wait(&mut body, &inline),
    }
    helper_return(&mut body, 0);
    body.instruction(&Instruction::End);
    body
}

fn fail_if_error(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    if indices.cooperative_helper_body {
        load_tag(body, 0, 0);
        body.instruction(&Instruction::If(BlockType::Empty));
        helper_return(body, 1);
        body.instruction(&Instruction::End);
    } else {
        emit_fail_if_retptr_error_inplace(body, indices);
    }
}

fn mem(offset: u64) -> MemArg {
    MemArg {
        offset,
        align: 0,
        memory_index: 0,
    }
}

fn load(body: &mut Function, address: i32, offset: u64) {
    body.instruction(&Instruction::I32Const(address));
    body.instruction(&Instruction::I32Load(mem(offset)));
}

fn set_zero(body: &mut Function, local: u32) {
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalSet(local));
}

fn load_tag(body: &mut Function, address: i32, offset: u64) {
    body.instruction(&Instruction::I32Const(address));
    body.instruction(&Instruction::I32Load8U(mem(offset)));
}

fn cancel_and_drop(body: &mut Function, indices: &DirectCoreFunctionIndices, handle: u32) {
    body.instruction(&Instruction::LocalGet(handle));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::Call(indices.waitable_join.unwrap()));
    body.instruction(&Instruction::LocalGet(handle));
    body.instruction(&Instruction::Call(indices.subtask_cancel.unwrap()));
    body.instruction(&Instruction::LocalSet(STATUS));
    // A callee may return normally while cancellation is being requested.
    body.instruction(&Instruction::LocalGet(STATUS));
    body.instruction(&Instruction::I32Const(RETURNED));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::LocalGet(STATUS));
    body.instruction(&Instruction::I32Const(CANCELLED));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::I32Or);
    // Backpressure can leave a call queued before parameter lowering/entry.
    // Cancelling that call resolves as START_CANCELLED and still needs drop.
    body.instruction(&Instruction::LocalGet(STATUS));
    body.instruction(&Instruction::I32Const(START_CANCELLED));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::I32Or);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::Unreachable);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalGet(handle));
    body.instruction(&Instruction::Call(indices.subtask_drop.unwrap()));
    set_zero(body, handle);
}

fn deadline_handle(body: &mut Function) {
    body.instruction(&Instruction::LocalGet(DEADLINE_STATUS));
    body.instruction(&Instruction::I32Const(4));
    body.instruction(&Instruction::I32ShrU);
}

fn close_deadline(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    deadline_handle(body);
    body.instruction(&Instruction::LocalTee(HANDLE));
    body.instruction(&Instruction::If(BlockType::Empty));
    cancel_and_drop(body, indices, HANDLE);
    body.instruction(&Instruction::End);
    set_zero(body, DEADLINE_STATUS);
}

fn close_wait(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    close_deadline(body, indices);
    for handle in [TARGET, TIMER] {
        body.instruction(&Instruction::LocalGet(handle));
        body.instruction(&Instruction::If(BlockType::Empty));
        cancel_and_drop(body, indices, handle);
        body.instruction(&Instruction::End);
    }
    body.instruction(&Instruction::LocalGet(SET));
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(SET));
    body.instruction(&Instruction::Call(indices.waitable_set_drop.unwrap()));
    set_zero(body, SET);
    body.instruction(&Instruction::End);
}

fn for_each_slot(body: &mut Function, emit: impl FnOnce(&mut Function)) {
    body.instruction(&Instruction::LocalGet(WINDOW_BEGIN));
    body.instruction(&Instruction::LocalSet(CURSOR));
    body.instruction(&Instruction::Block(BlockType::Empty));
    body.instruction(&Instruction::Loop(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(CURSOR));
    body.instruction(&Instruction::LocalGet(WINDOW_END));
    body.instruction(&Instruction::I32GeU);
    body.instruction(&Instruction::BrIf(1));
    emit(body);
    body.instruction(&Instruction::LocalGet(CURSOR));
    body.instruction(&Instruction::I32Const(super::DIRECT_PSPLIT_SLOT_STRIDE));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(CURSOR));
    body.instruction(&Instruction::Br(0));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
}

fn clear_slot_handle(body: &mut Function) {
    body.instruction(&Instruction::LocalGet(CURSOR));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Store(mem(
        super::DIRECT_PSPLIT_SLOT_SUBTASK_OFFSET as u64,
    )));
}

fn close_all(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    if indices.subtask_cancel.is_none() {
        return;
    }
    close_wait(body, indices);
    body.instruction(&Instruction::LocalGet(WINDOW_ACTIVE));
    body.instruction(&Instruction::If(BlockType::Empty));
    for_each_slot(body, |body| {
        body.instruction(&Instruction::LocalGet(CURSOR));
        body.instruction(&Instruction::I32Load(mem(
            super::DIRECT_PSPLIT_SLOT_SUBTASK_OFFSET as u64,
        )));
        body.instruction(&Instruction::LocalTee(HANDLE));
        body.instruction(&Instruction::If(BlockType::Empty));
        cancel_and_drop(body, indices, HANDLE);
        clear_slot_handle(body);
        body.instruction(&Instruction::End);
    });
    emit_window_close(body, indices);
    body.instruction(&Instruction::End);
}

fn poll_error(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    load_tag(body, POLL, 0);
    body.instruction(&Instruction::If(BlockType::Empty));
    close_all(body, indices);
    // Only after resolution may an error replace the Agent's result area.
    for offset in [0, 4, 8] {
        body.instruction(&Instruction::I32Const(0));
        load(body, POLL, offset);
        body.instruction(&Instruction::I32Store(mem(offset)));
    }
    fail_if_error(body, indices);
    body.instruction(&Instruction::End);
}

fn remember_signal(body: &mut Function, address: i32, first_offset: u64) {
    for (n, local) in [SIGNAL_PTR, SIGNAL_LEN, COMMAND_PTR, COMMAND_LEN]
        .into_iter()
        .enumerate()
    {
        load(body, address, first_offset + n as u64 * 4);
        body.instruction(&Instruction::LocalSet(local));
    }
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::LocalSet(SIGNAL_PENDING));
}

fn acknowledge(body: &mut Function, indices: &DirectCoreFunctionIndices, cancel: bool) {
    for local in [SIGNAL_PTR, SIGNAL_LEN, COMMAND_PTR, COMMAND_LEN] {
        body.instruction(&Instruction::LocalGet(local));
    }
    push_retptr_arg(body);
    body.instruction(&Instruction::Call(indices.runtime_handle_checkpoint_signal));
    fail_if_error(body, indices);
    set_zero(body, SIGNAL_PENDING);
    load_tag(body, 0, 4);
    body.instruction(&Instruction::If(BlockType::Empty));
    if indices.cooperative_helper_body {
        helper_return(body, 2);
    } else {
        emit_entry_suspend_return(body, indices);
    }
    if cancel {
        // A rejected Cancel receipt cannot resume a graph whose calls were cancelled.
        body.instruction(&Instruction::Else);
        body.instruction(&Instruction::Unreachable);
    }
    body.instruction(&Instruction::End);
}

fn act_on_cancel(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    body.instruction(&Instruction::LocalGet(SIGNAL_PENDING));
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(SIGNAL_LEN));
    body.instruction(&Instruction::I32Const(6));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(SIGNAL_PTR));
    body.instruction(&Instruction::I32Load(mem(0)));
    body.instruction(&Instruction::I32Const(i32::from_le_bytes(*b"canc")));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::LocalGet(SIGNAL_PTR));
    body.instruction(&Instruction::I32Load16U(mem(4)));
    body.instruction(&Instruction::I32Const(u16::from_le_bytes(*b"el") as i32));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::I32And);
    body.instruction(&Instruction::If(BlockType::Empty));
    close_all(body, indices);
    acknowledge(body, indices, true);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
}

fn poll(body: &mut Function, indices: &DirectCoreFunctionIndices, heartbeat: bool) {
    if heartbeat {
        body.instruction(&Instruction::I32Const(POLL));
        body.instruction(&Instruction::Call(indices.runtime_heartbeat));
        poll_error(body, indices);
    }
    body.instruction(&Instruction::I32Const(POLL));
    body.instruction(&Instruction::Call(indices.runtime_poll_signal));
    poll_error(body, indices);
    // A rate-limited None is not a withdrawal: retain the command until an
    // explicit receipt accepts it or rejects it as stale/superseded.
    load_tag(body, POLL, 4);
    body.instruction(&Instruction::If(BlockType::Empty));
    remember_signal(body, POLL, 8);
    body.instruction(&Instruction::End);
    act_on_cancel(body, indices);
}

/// Called after a checkpoint (result at zero), before another import can
/// overwrite its signal record. Pause/shutdown wait for the window's durable
/// assembly boundary; Cancel cleans all active handles before acknowledgement.
pub(super) fn emit_checkpoint_signal(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    if call_helper(body, indices, Helper::Checkpoint) {
        return;
    }
    load_tag(body, 0, 0);
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    load_tag(body, 0, super::DIRECT_CHECKPOINT_PENDING_SIGNAL_TAG_OFFSET);
    body.instruction(&Instruction::If(BlockType::Empty));
    remember_signal(body, 0, super::DIRECT_CHECKPOINT_SIGNAL_TYPE_PTR_OFFSET);
    body.instruction(&Instruction::End);
    emit_retained_boundary(body, indices);
    body.instruction(&Instruction::End);
}

pub(super) fn emit_retained_boundary(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    if call_helper(body, indices, Helper::Boundary) {
        return;
    }
    act_on_cancel(body, indices);
    body.instruction(&Instruction::LocalGet(DEFER_BOUNDARY));
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::LocalGet(SIGNAL_PENDING));
    body.instruction(&Instruction::I32And);
    body.instruction(&Instruction::If(BlockType::Empty));
    acknowledge(body, indices, false);
    body.instruction(&Instruction::End);
}

/// Stack input: end address of the existing, zero-initialized slot array.
pub(super) fn emit_window_open(body: &mut Function) {
    body.instruction(&Instruction::LocalSet(WINDOW_END));
    body.instruction(&Instruction::LocalGet(super::DIRECT_PSPLIT_SLOTS_LOCAL));
    body.instruction(&Instruction::LocalSet(WINDOW_BEGIN));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::LocalSet(WINDOW_ACTIVE));
    body.instruction(&Instruction::LocalGet(DEFER_BOUNDARY));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(DEFER_BOUNDARY));
    set_zero(body, WINDOW_TIMER);
}

/// Close an already drained window, retaining pause intent through assembly.
pub(super) fn emit_window_close(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    body.instruction(&Instruction::LocalGet(WINDOW_TIMER));
    body.instruction(&Instruction::If(BlockType::Empty));
    cancel_and_drop(body, indices, WINDOW_TIMER);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalGet(super::DIRECT_PSPLIT_WS_LOCAL));
    body.instruction(&Instruction::Call(indices.waitable_set_drop.unwrap()));
    set_zero(body, WINDOW_ACTIVE);
}

pub(super) fn emit_window_boundary(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    release_window_boundary(body);
    if !indices.omit_runtime {
        super::checkpoint::emit_check_signals_and_suspend(body, indices);
    }
}

fn release_window_boundary(body: &mut Function) {
    body.instruction(&Instruction::LocalGet(DEFER_BOUNDARY));
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::I32Sub);
    body.instruction(&Instruction::LocalSet(DEFER_BOUNDARY));
}

/// Select an enclosing deadline in the entry function, where its owner/frame
/// live. The helper owns only this wait's timer and the active window's handles.
/// Resolve the timer on every return: branch assembly can itself await I/O and
/// must not inherit or overwrite a live timer from the preceding window wait.
pub(super) fn emit_scoped_window_wait(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    failure_target: Option<super::DirectFailureTarget>,
) {
    if indices.monotonic_now.is_some() {
        super::deadline_scope::arm(body, indices, false);
    }
    emit_window_wait(body, indices);
    if indices.monotonic_now.is_some() {
        body.instruction(&Instruction::LocalGet(TIMED_OUT));
        body.instruction(&Instruction::If(BlockType::Empty));
        super::deadline_scope::select(body);
        body.instruction(&Instruction::End);
        super::deadline_scope::propagate(body, indices, failure_target);
    }
}

/// Check around synchronous preparation/assembly without waiting for peers.
/// When expiry is already known, clean the whole owned window before unwind;
/// otherwise a fast branch could keep launching work while its sibling hangs.
pub(super) fn emit_window_deadline_boundary(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    failure_target: Option<super::DirectFailureTarget>,
) {
    if indices.monotonic_now.is_none() {
        return;
    }
    super::deadline_scope::choose(body, indices, false);
    body.instruction(&Instruction::LocalGet(super::agent_deadline::REMAINING));
    body.instruction(&Instruction::I64Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    assert!(call_helper(body, indices, Helper::WindowCancel));
    super::deadline_scope::select(body);
    super::deadline_scope::propagate(body, indices, failure_target.map(|target| target.nested(1)));
    body.instruction(&Instruction::End);
}

fn cancel_window_deadline(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    if !indices.omit_runtime {
        poll(body, indices, false);
    }
    close_all(body, indices);
    // Recovery resumes this invocation, so balance the window's deferral.
    // Retained pause/shutdown intent remains pending until a safe boundary.
    release_window_boundary(body);
    helper_return(body, 4);
}

/// A timed-out preparation Await owns only its lookup handle. Resolve the
/// enclosing window's peers before forwarding the selected scope's reason.
pub(super) fn emit_window_preparation_timeout(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    failure_target: Option<super::DirectFailureTarget>,
) {
    if indices.monotonic_now.is_none() {
        return;
    }
    body.instruction(&Instruction::LocalGet(TIMED_OUT));
    body.instruction(&Instruction::If(BlockType::Empty));
    assert!(call_helper(body, indices, Helper::WindowCancel));
    super::deadline_scope::propagate(body, indices, failure_target.map(|target| target.nested(1)));
    body.instruction(&Instruction::End);
}

/// Wait for one event. A polling timer is internal to this wait and never
/// decrements the window's call count or advances a branch cursor.
pub(super) fn emit_window_wait(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    if call_helper(body, indices, Helper::WindowWait) {
        return;
    }
    deadline_handle(body);
    body.instruction(&Instruction::LocalTee(HANDLE));
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(HANDLE));
    body.instruction(&Instruction::LocalGet(super::DIRECT_PSPLIT_WS_LOCAL));
    body.instruction(&Instruction::Call(indices.waitable_join.unwrap()));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::Loop(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(DEADLINE_STATUS));
    body.instruction(&Instruction::If(BlockType::Empty));
    // Deliver already-ready calls before selecting expiry, regardless of the
    // notification order. The caller drains/assembles one ordinary event per
    // return; internal timer events never decrement its pending-call count.
    body.instruction(&Instruction::Block(BlockType::Empty));
    body.instruction(&Instruction::Loop(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(super::DIRECT_PSPLIT_WS_LOCAL));
    body.instruction(&Instruction::I32Const(DIRECT_PSPLIT_EVENT_OFFSET));
    body.instruction(&Instruction::Call(indices.waitable_set_poll.unwrap()));
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::BrIf(1));
    observe_window_event(body, indices);
    body.instruction(&Instruction::Br(0));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalGet(DEADLINE_STATUS));
    body.instruction(&Instruction::I32Const(RETURNED));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    cancel_window_deadline(body, indices);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    if !indices.omit_runtime {
        body.instruction(&Instruction::LocalGet(WINDOW_TIMER));
        body.instruction(&Instruction::I32Eqz);
        body.instruction(&Instruction::If(BlockType::Empty));
        poll(body, indices, true);
        body.instruction(&Instruction::I64Const(POLL_INTERVAL_MS));
        body.instruction(&Instruction::Call(indices.timer_sleep_async.unwrap()));
        body.instruction(&Instruction::LocalSet(STATUS));
        body.instruction(&Instruction::LocalGet(STATUS));
        body.instruction(&Instruction::I32Const(15));
        body.instruction(&Instruction::I32And);
        body.instruction(&Instruction::I32Const(RETURNED));
        body.instruction(&Instruction::I32Ne);
        body.instruction(&Instruction::If(BlockType::Empty));
        body.instruction(&Instruction::LocalGet(STATUS));
        body.instruction(&Instruction::I32Const(4));
        body.instruction(&Instruction::I32ShrU);
        body.instruction(&Instruction::LocalTee(WINDOW_TIMER));
        body.instruction(&Instruction::LocalGet(super::DIRECT_PSPLIT_WS_LOCAL));
        body.instruction(&Instruction::Call(indices.waitable_join.unwrap()));
        body.instruction(&Instruction::End);
        body.instruction(&Instruction::End);
        body.instruction(&Instruction::LocalGet(WINDOW_TIMER));
        body.instruction(&Instruction::If(BlockType::Empty));
    }
    body.instruction(&Instruction::LocalGet(super::DIRECT_PSPLIT_WS_LOCAL));
    body.instruction(&Instruction::I32Const(DIRECT_PSPLIT_EVENT_OFFSET));
    body.instruction(&Instruction::Call(indices.waitable_set_wait.unwrap()));
    handle_wait_event(body, indices);
    observe_window_event(body, indices);
    if !indices.omit_runtime {
        // An eager lifecycle timer needs another poll, not an
        // uninterruptible wait on an otherwise empty waitable set.
        body.instruction(&Instruction::End);
    }
    body.instruction(&Instruction::Br(0));
    body.instruction(&Instruction::End);
}

fn observe_window_event(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    load(body, DIRECT_PSPLIT_EVENT_OFFSET, 4);
    body.instruction(&Instruction::I32Const(RETURNED));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    load(body, DIRECT_PSPLIT_EVENT_OFFSET, 0);
    deadline_handle(body);
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    load(body, DIRECT_PSPLIT_EVENT_OFFSET, 0);
    body.instruction(&Instruction::Call(indices.subtask_drop.unwrap()));
    body.instruction(&Instruction::I32Const(RETURNED));
    body.instruction(&Instruction::LocalSet(DEADLINE_STATUS));
    body.instruction(&Instruction::Else);
    load(body, DIRECT_PSPLIT_EVENT_OFFSET, 0);
    body.instruction(&Instruction::LocalGet(WINDOW_TIMER));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(WINDOW_TIMER));
    body.instruction(&Instruction::Call(indices.subtask_drop.unwrap()));
    set_zero(body, WINDOW_TIMER);
    body.instruction(&Instruction::Else);
    close_deadline(body, indices);
    helper_return(body, 0);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
}

/// Clear a resolved call's slot before its handle can be recycled by the engine.
pub(super) fn emit_forget_returned(body: &mut Function) {
    for_each_slot(body, |body| {
        body.instruction(&Instruction::LocalGet(CURSOR));
        body.instruction(&Instruction::I32Load(mem(
            super::DIRECT_PSPLIT_SLOT_SUBTASK_OFFSET as u64,
        )));
        load(body, DIRECT_PSPLIT_EVENT_OFFSET, 0);
        body.instruction(&Instruction::I32Eq);
        body.instruction(&Instruction::If(BlockType::Empty));
        clear_slot_handle(body);
        body.instruction(&Instruction::End);
    });
}

pub(super) fn emit_poll_before_call(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    if call_helper(body, indices, Helper::Poll) {
        return;
    }
    if !indices.omit_runtime {
        poll(body, indices, false);
    }
}

/// Await a timer duration (milliseconds) already on the stack. Non-durable
/// Agent, Embed and Split retries share this standard call/wait/cleanup path.
pub(super) fn emit_timer_wait(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    body.instruction(&Instruction::Call(
        indices
            .timer_sleep_async
            .expect("timer wait requires a timer import"),
    ));
    emit_await_call(body, indices);
}

/// Consume an async-lowered invoke's packed status, preserving its result at 0.
pub(super) fn emit_await_call(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    set_zero(body, TIMED_OUT);
    body.instruction(&Instruction::LocalSet(STATUS));
    body.instruction(&Instruction::LocalGet(STATUS));
    body.instruction(&Instruction::I32Const(15));
    body.instruction(&Instruction::I32And);
    body.instruction(&Instruction::I32Const(RETURNED));
    body.instruction(&Instruction::I32Ne);
    body.instruction(&Instruction::If(BlockType::Empty));
    // Eagerly completed calls need neither a wait nor a state transfer.
    if call_helper(body, indices, Helper::Await) {
        body.instruction(&Instruction::Else);
        close_deadline(body, indices);
        body.instruction(&Instruction::End);
        return;
    }
    body.instruction(&Instruction::LocalGet(STATUS));
    body.instruction(&Instruction::I32Const(4));
    body.instruction(&Instruction::I32ShrU);
    body.instruction(&Instruction::LocalSet(TARGET));
    body.instruction(&Instruction::Call(indices.waitable_set_new.unwrap()));
    body.instruction(&Instruction::LocalSet(SET));
    body.instruction(&Instruction::LocalGet(TARGET));
    body.instruction(&Instruction::LocalGet(SET));
    body.instruction(&Instruction::Call(indices.waitable_join.unwrap()));
    set_zero(body, TIMER);
    deadline_handle(body);
    body.instruction(&Instruction::LocalTee(HANDLE));
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(HANDLE));
    body.instruction(&Instruction::LocalGet(SET));
    body.instruction(&Instruction::Call(indices.waitable_join.unwrap()));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::Block(BlockType::Empty));
    body.instruction(&Instruction::Loop(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(TARGET));
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::BrIf(1));
    body.instruction(&Instruction::LocalGet(DEADLINE_STATUS));
    body.instruction(&Instruction::If(BlockType::Empty));
    // Drain all ready events before selecting expiry. An already-ready target
    // wins the deadline tie, independently of waitable-set notification order.
    body.instruction(&Instruction::Block(BlockType::Empty));
    body.instruction(&Instruction::Loop(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(SET));
    body.instruction(&Instruction::I32Const(DIRECT_PSPLIT_EVENT_OFFSET));
    body.instruction(&Instruction::Call(indices.waitable_set_poll.unwrap()));
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::BrIf(1));
    observe_await_event(body, indices);
    body.instruction(&Instruction::Br(0));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalGet(TARGET));
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::BrIf(2));
    body.instruction(&Instruction::LocalGet(DEADLINE_STATUS));
    body.instruction(&Instruction::I32Const(RETURNED));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    if !indices.omit_runtime {
        poll(body, indices, false);
    }
    // Only this operation is owned by the deadline. An enclosing window's
    // unrelated calls remain live. Root/parent cancellation uses close_all.
    close_wait(body, indices);
    helper_return(body, 4);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    if !indices.omit_runtime {
        body.instruction(&Instruction::LocalGet(TIMER));
        body.instruction(&Instruction::I32Eqz);
        body.instruction(&Instruction::If(BlockType::Empty));
        poll(body, indices, true);
        body.instruction(&Instruction::I64Const(POLL_INTERVAL_MS));
        body.instruction(&Instruction::Call(indices.timer_sleep_async.unwrap()));
        body.instruction(&Instruction::LocalSet(STATUS));
        body.instruction(&Instruction::LocalGet(STATUS));
        body.instruction(&Instruction::I32Const(15));
        body.instruction(&Instruction::I32And);
        body.instruction(&Instruction::I32Const(RETURNED));
        body.instruction(&Instruction::I32Eq);
        // An eagerly completed timer is already due; poll again without waiting.
        body.instruction(&Instruction::BrIf(1));
        body.instruction(&Instruction::LocalGet(STATUS));
        body.instruction(&Instruction::I32Const(4));
        body.instruction(&Instruction::I32ShrU);
        body.instruction(&Instruction::LocalTee(TIMER));
        body.instruction(&Instruction::LocalGet(SET));
        body.instruction(&Instruction::Call(indices.waitable_join.unwrap()));
        body.instruction(&Instruction::End);
    }
    body.instruction(&Instruction::LocalGet(SET));
    body.instruction(&Instruction::I32Const(DIRECT_PSPLIT_EVENT_OFFSET));
    body.instruction(&Instruction::Call(indices.waitable_set_wait.unwrap()));
    handle_wait_event(body, indices);
    observe_await_event(body, indices);
    body.instruction(&Instruction::Br(0));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    close_wait(body, indices);
    body.instruction(&Instruction::Else);
    // The operation completed eagerly; its deadline still needs resolution.
    close_deadline(body, indices);
    body.instruction(&Instruction::End);
}

fn observe_await_event(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    load(body, DIRECT_PSPLIT_EVENT_OFFSET, 4);
    body.instruction(&Instruction::I32Const(RETURNED));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    load(body, DIRECT_PSPLIT_EVENT_OFFSET, 0);
    body.instruction(&Instruction::Call(indices.subtask_drop.unwrap()));
    load(body, DIRECT_PSPLIT_EVENT_OFFSET, 0);
    body.instruction(&Instruction::LocalGet(TARGET));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    set_zero(body, TARGET);
    body.instruction(&Instruction::Else);
    load(body, DIRECT_PSPLIT_EVENT_OFFSET, 0);
    deadline_handle(body);
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::I32Const(RETURNED));
    body.instruction(&Instruction::LocalSet(DEADLINE_STATUS));
    body.instruction(&Instruction::Else);
    set_zero(body, TIMER);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
}

/// Cooperate at an iteration boundary. A callable workflow receives its
/// parent's cancellation through the canonical yield; a root observes its
/// lifecycle signal. Both unwind the same guest-owned pending-call state.
/// This bounds cooperation by one emitted iteration, not by a native call or
/// arbitrary loop inside an Agent/stdlib function.
pub(super) fn emit_iteration_boundary(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    if let Some(yield_index) = indices.thread_yield {
        body.instruction(&Instruction::Call(yield_index));
        body.instruction(&Instruction::If(BlockType::Empty));
        close_all(body, indices);
        emit_entry_cancel_return(body);
        body.instruction(&Instruction::End);
    }
    if !indices.omit_runtime {
        emit_poll_before_call(body, indices);
        emit_retained_boundary(body, indices);
    }
}

/// Opens an if for a safe consuming legacy poll. Pending windows only observe.
pub(super) fn emit_if_safe_boundary(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    emit_iteration_boundary(body, indices);
    body.instruction(&Instruction::LocalGet(DEFER_BOUNDARY));
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
}

#[cfg(test)]
#[path = "cooperative_wait_tests.rs"]
mod tests;
