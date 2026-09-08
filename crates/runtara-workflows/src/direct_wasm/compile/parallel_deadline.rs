//! Per-Agent budgets in the existing guest-owned parallel window.
//! One standard timer selects the next expiring Agent. Ready calls are buffered
//! in their existing slots before expiry is chosen, preserving completion ties.
use super::super::{DirectCoreStaticData, agent_deadline};
use super::*;

pub(crate) const ENABLED: u32 = 179;
pub(crate) const OWNER: u32 = 180;
pub(crate) const TIMER_STATUS: u32 = 181;
const ACTIVE: u64 = 176;
const READY: u64 = 180;
const START: u64 = 184;
const BUDGET: u64 = 192;
const ERROR: u64 = 200;
const ALARM: u64 = 204; // occupies the existing slot padding; stride stays 208
const BEST: u32 = agent_deadline::DEADLINE;

fn slot_load(body: &mut Function, slot: u32, offset: u64) {
    body.instruction(&Instruction::LocalGet(slot));
    body.instruction(&Instruction::I32Load(mem(offset)));
}
fn slot_store(body: &mut Function, slot: u32, offset: u64, value: i32) {
    body.instruction(&Instruction::LocalGet(slot));
    body.instruction(&Instruction::I32Const(value));
    body.instruction(&Instruction::I32Store(mem(offset)));
}

pub(crate) fn reset_slot(body: &mut Function, slot: u32) {
    slot_store(body, slot, ACTIVE, 0);
    slot_store(body, slot, READY, 0);
}

/// Call after input validation/cache lookup and before connection preparation.
#[allow(clippy::too_many_arguments)]
pub(crate) fn begin(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    data: &DirectCoreStaticData,
    agent: u32,
    step: &str,
    source: (u32, u32),
    durable: bool,
    slot: u32,
) {
    reset_slot(body, slot);
    let Some(timeout) = data.agent_timeout(agent) else {
        return;
    };
    agent_deadline::enter(
        body,
        indices,
        &data.agent_deadline_state_error,
        data.step_id(step).expect("planned Agent"),
        source,
        timeout,
        durable,
    );
    for (offset, local) in [
        (START, agent_deadline::START_NS),
        (BUDGET, agent_deadline::BUDGET_MS),
    ] {
        body.instruction(&Instruction::LocalGet(slot));
        body.instruction(&Instruction::LocalGet(local));
        body.instruction(&Instruction::I64Store(mem(offset)));
    }
    slot_store(body, slot, ERROR, data.agent_timeout_error.offset);
    slot_store(body, slot, ACTIVE, 1);
    body.instruction(&Instruction::I32Const(1));
    body.instruction(&Instruction::LocalSet(ENABLED));
}

/// This belongs to the call, independently of whichever wait currently drives
/// the window. Its first instruction may prevent the scheduler from returning.
pub(crate) fn start_call_alarm(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    own: bool,
    slot: u32,
) {
    if indices.monotonic_now.is_none() {
        return;
    }
    // Reusing a live slot would orphan its previous alarm.
    slot_load(body, slot, ALARM);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::Unreachable);
    body.instruction(&Instruction::End);
    super::super::deadline_scope::arm_call_alarm(body, indices, own, HANDLE);
    body.instruction(&Instruction::LocalGet(slot));
    body.instruction(&Instruction::LocalGet(HANDLE));
    body.instruction(&Instruction::I32Store(mem(ALARM)));
}

pub(crate) fn close_call_alarm(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    slot: u32,
) {
    if indices.timer_abort_async.is_none() {
        return;
    }
    slot_load(body, slot, ALARM);
    body.instruction(&Instruction::LocalTee(HANDLE));
    body.instruction(&Instruction::If(BlockType::Empty));
    cancel_and_drop(body, indices, HANDLE);
    slot_store(body, slot, ALARM, 0);
    body.instruction(&Instruction::End);
}

pub(crate) fn close_eager_call_alarm(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    slot: u32,
    status: u32,
) {
    body.instruction(&Instruction::LocalGet(status));
    body.instruction(&Instruction::I32Const(15));
    body.instruction(&Instruction::I32And);
    body.instruction(&Instruction::I32Const(RETURNED));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    close_call_alarm(body, indices, slot);
    body.instruction(&Instruction::End);
}

pub(crate) fn timer_handle(body: &mut Function) {
    body.instruction(&Instruction::LocalGet(TIMER_STATUS));
    body.instruction(&Instruction::I32Const(4));
    body.instruction(&Instruction::I32ShrU);
}

pub(crate) fn close_timer(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    if indices.subtask_cancel.is_none() {
        return;
    }
    timer_handle(body);
    body.instruction(&Instruction::LocalTee(HANDLE));
    body.instruction(&Instruction::If(BlockType::Empty));
    cancel_and_drop(body, indices, HANDLE);
    body.instruction(&Instruction::End);
    set_zero(body, TIMER_STATUS);
    set_zero(body, OWNER);
}

pub(crate) fn arm(body: &mut Function, indices: &DirectCoreFunctionIndices, set: u32) {
    if indices.monotonic_now.is_none() {
        return;
    }
    body.instruction(&Instruction::LocalGet(ENABLED));
    body.instruction(&Instruction::LocalGet(WINDOW_ACTIVE));
    body.instruction(&Instruction::I32And);
    body.instruction(&Instruction::LocalGet(TIMER_STATUS));
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::I32And);
    body.instruction(&Instruction::If(BlockType::Empty));
    set_zero(body, OWNER);
    body.instruction(&Instruction::I64Const(-1));
    body.instruction(&Instruction::LocalSet(BEST));
    for_each_slot(body, |body| {
        slot_load(body, CURSOR, ACTIVE);
        slot_load(
            body,
            CURSOR,
            super::super::DIRECT_PSPLIT_SLOT_SUBTASK_OFFSET as u64,
        );
        body.instruction(&Instruction::I32Eqz);
        body.instruction(&Instruction::I32Eqz);
        body.instruction(&Instruction::I32And);
        body.instruction(&Instruction::If(BlockType::Empty));
        for (offset, local) in [
            (START, agent_deadline::START_NS),
            (BUDGET, agent_deadline::BUDGET_MS),
        ] {
            body.instruction(&Instruction::LocalGet(CURSOR));
            body.instruction(&Instruction::I64Load(mem(offset)));
            body.instruction(&Instruction::LocalSet(local));
        }
        agent_deadline::remaining(body, indices);
        body.instruction(&Instruction::LocalGet(agent_deadline::REMAINING));
        body.instruction(&Instruction::LocalGet(BEST));
        body.instruction(&Instruction::I64LtU);
        body.instruction(&Instruction::LocalGet(OWNER));
        body.instruction(&Instruction::I32Eqz);
        body.instruction(&Instruction::I32Or);
        body.instruction(&Instruction::If(BlockType::Empty));
        body.instruction(&Instruction::LocalGet(agent_deadline::REMAINING));
        body.instruction(&Instruction::LocalSet(BEST));
        body.instruction(&Instruction::LocalGet(CURSOR));
        body.instruction(&Instruction::LocalSet(OWNER));
        body.instruction(&Instruction::End);
        body.instruction(&Instruction::End);
    });
    body.instruction(&Instruction::LocalGet(OWNER));
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(BEST));
    body.instruction(&Instruction::Call(indices.timer_sleep_async.unwrap()));
    body.instruction(&Instruction::LocalSet(TIMER_STATUS));
    timer_handle(body);
    body.instruction(&Instruction::LocalTee(HANDLE));
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(HANDLE));
    body.instruction(&Instruction::LocalGet(set));
    body.instruction(&Instruction::Call(indices.waitable_join.unwrap()));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
}

/// Untimed windows use the same alarm lifetime when an enclosing scope supplied
/// the budget. Dispose before returning to assembly or another native import.
pub(crate) fn close_returned_alarm(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    for_each_slot(body, |body| {
        slot_load(
            body,
            CURSOR,
            super::super::DIRECT_PSPLIT_SLOT_SUBTASK_OFFSET as u64,
        );
        load(body, DIRECT_PSPLIT_EVENT_OFFSET, 0);
        body.instruction(&Instruction::I32Eq);
        body.instruction(&Instruction::If(BlockType::Empty));
        close_call_alarm(body, indices, CURSOR);
        body.instruction(&Instruction::End);
    });
}

/// Detach a ready call so a second poll cannot deliver the same event. Its
/// resolved handle remains in the slot until the normal scheduler drops it.
pub(crate) fn remember_returned(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    load(body, DIRECT_PSPLIT_EVENT_OFFSET, 0);
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::Call(indices.waitable_join.unwrap()));
    for_each_slot(body, |body| {
        slot_load(
            body,
            CURSOR,
            super::super::DIRECT_PSPLIT_SLOT_SUBTASK_OFFSET as u64,
        );
        load(body, DIRECT_PSPLIT_EVENT_OFFSET, 0);
        body.instruction(&Instruction::I32Eq);
        body.instruction(&Instruction::If(BlockType::Empty));
        close_call_alarm(body, indices, CURSOR);
        slot_store(body, CURSOR, ACTIVE, 0);
        slot_store(body, CURSOR, READY, 1);
        body.instruction(&Instruction::LocalGet(CURSOR));
        body.instruction(&Instruction::LocalGet(OWNER));
        body.instruction(&Instruction::I32Eq);
        body.instruction(&Instruction::If(BlockType::Empty));
        close_timer(body, indices);
        body.instruction(&Instruction::End);
        body.instruction(&Instruction::End);
    });
}

/// Write the same canonical Agent timeout error into this slot's result area.
pub(crate) fn error(body: &mut Function, slot: u32) {
    const RESULT: u64 = super::super::DIRECT_PSPLIT_SLOT_RESULT_OFFSET as u64;
    body.instruction(&Instruction::LocalGet(slot));
    body.instruction(&Instruction::I32Const(RESULT as i32));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Const(80));
    body.instruction(&Instruction::MemoryFill(0));
    slot_store(body, slot, RESULT, 1);
    let mut prefix = 0;
    for (offset, text) in [8, 16, 24, 32]
        .into_iter()
        .zip(crate::direct_wasm::static_data::AGENT_TIMEOUT_FIELDS)
    {
        body.instruction(&Instruction::LocalGet(slot));
        slot_load(body, slot, ERROR);
        body.instruction(&Instruction::I32Const(prefix));
        body.instruction(&Instruction::I32Add);
        body.instruction(&Instruction::I32Store(mem(RESULT + offset)));
        slot_store(body, slot, RESULT + offset + 4, text.len() as i32);
        prefix += text.len() as i32;
    }
    // Memoized ordinary Agent lowering consumes the existing ready-slot state.
    slot_store(body, slot, 0, 1);
    slot_store(body, slot, ACTIVE, 0);
}

fn deliver(body: &mut Function, indices: &DirectCoreFunctionIndices, slot: u32) {
    body.instruction(&Instruction::I32Const(DIRECT_PSPLIT_EVENT_OFFSET));
    slot_load(
        body,
        slot,
        super::super::DIRECT_PSPLIT_SLOT_SUBTASK_OFFSET as u64,
    );
    body.instruction(&Instruction::I32Store(mem(0)));
    body.instruction(&Instruction::I32Const(DIRECT_PSPLIT_EVENT_OFFSET));
    body.instruction(&Instruction::I32Const(RETURNED));
    body.instruction(&Instruction::I32Store(mem(4)));
    close_timer(body, indices);
    close_deadline(body, indices);
    close_alarm(body, indices);
    helper_return(body, 0);
}

/// Select a due Agent without returning from the current preparation wait.
/// Its resolved handle/result stay in the ordinary slot for later assembly.
pub(crate) fn select_expired(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    if !indices.omit_runtime {
        poll(body, indices, false);
    }
    body.instruction(&Instruction::LocalGet(OWNER));
    body.instruction(&Instruction::LocalSet(CURSOR));
    slot_load(
        body,
        CURSOR,
        super::super::DIRECT_PSPLIT_SLOT_SUBTASK_OFFSET as u64,
    );
    body.instruction(&Instruction::LocalSet(HANDLE));
    body.instruction(&Instruction::LocalGet(HANDLE));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::Call(indices.waitable_join.unwrap()));
    body.instruction(&Instruction::LocalGet(HANDLE));
    body.instruction(&Instruction::Call(indices.subtask_cancel.unwrap()));
    body.instruction(&Instruction::LocalSet(STATUS));
    // Leave the resolved call handle for the ordinary scheduler's single drop.
    for outcome in [RETURNED, START_CANCELLED, CANCELLED] {
        body.instruction(&Instruction::LocalGet(STATUS));
        body.instruction(&Instruction::I32Const(outcome));
        body.instruction(&Instruction::I32Eq);
        if outcome != RETURNED {
            body.instruction(&Instruction::I32Or);
        }
    }
    body.instruction(&Instruction::I32Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::Unreachable);
    body.instruction(&Instruction::End);
    close_call_alarm(body, indices, CURSOR);
    error(body, CURSOR);
    slot_store(body, CURSOR, READY, 1);
    close_timer(body, indices);
}

/// A pending-window event observed by either the main window or a preparation
/// wait. It must not drop the call handle before the scheduler consumes it.
pub(crate) fn observe_event(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    load(body, DIRECT_PSPLIT_EVENT_OFFSET, 0);
    timer_handle(body);
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    load(body, DIRECT_PSPLIT_EVENT_OFFSET, 0);
    body.instruction(&Instruction::Call(indices.subtask_drop.unwrap()));
    body.instruction(&Instruction::I32Const(RETURNED));
    body.instruction(&Instruction::LocalSet(TIMER_STATUS));
    body.instruction(&Instruction::Else);
    remember_returned(body, indices);
    body.instruction(&Instruction::End);
}

pub(crate) fn select_or_deliver(body: &mut Function, indices: &DirectCoreFunctionIndices) {
    body.instruction(&Instruction::LocalGet(ENABLED));
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(TIMER_STATUS));
    body.instruction(&Instruction::I32Const(RETURNED));
    body.instruction(&Instruction::I32Eq);
    body.instruction(&Instruction::If(BlockType::Empty));
    select_expired(body, indices);
    deliver(body, indices, CURSOR);
    body.instruction(&Instruction::End);
    for_each_slot(body, |body| {
        slot_load(body, CURSOR, READY);
        body.instruction(&Instruction::If(BlockType::Empty));
        deliver(body, indices, CURSOR);
        body.instruction(&Instruction::End);
    });
    body.instruction(&Instruction::End);
}

/// A completed preparation wait has already selected the earliest scope. Own
/// expiry memoizes this Agent's error; an enclosing expiry still unwinds its
/// entire window. skip_depth is the launch block's current relative depth.
pub(crate) fn preparation_done(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    slot: u32,
    skip_depth: u32,
    failure: Option<super::super::DirectFailureTarget>,
) {
    body.instruction(&Instruction::LocalGet(TIMED_OUT));
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(
        super::super::deadline_scope::SELECTED,
    ));
    body.instruction(&Instruction::I64Eqz);
    body.instruction(&Instruction::If(BlockType::Empty));
    emit_poll_before_call(body, indices);
    error(body, slot);
    body.instruction(&Instruction::Br(skip_depth + 2));
    body.instruction(&Instruction::Else);
    emit_window_preparation_timeout(body, indices, failure.map(|t| t.nested(2)));
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::End);
}

pub(crate) fn check_before_io(
    body: &mut Function,
    indices: &DirectCoreFunctionIndices,
    slot: u32,
    skip_depth: u32,
    failure: Option<super::super::DirectFailureTarget>,
) {
    super::super::deadline_scope::choose(body, indices, true);
    body.instruction(&Instruction::LocalGet(agent_deadline::REMAINING));
    body.instruction(&Instruction::I64Eqz);
    body.instruction(&Instruction::LocalSet(TIMED_OUT));
    body.instruction(&Instruction::LocalGet(TIMED_OUT));
    body.instruction(&Instruction::If(BlockType::Empty));
    super::super::deadline_scope::select(body);
    body.instruction(&Instruction::End);
    preparation_done(body, indices, slot, skip_depth, failure);
}
