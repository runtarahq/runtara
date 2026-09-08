//! Drive the production WindowWait helper with independently ordered ready
//! notifications. Component-level cancellation semantics are tested separately;
//! this fixture controls the scheduling boundary without wall-clock races.
use super::*;

const FIRST: usize = 1024;
const SECOND: usize = FIRST + DIRECT_PSPLIT_SLOT_STRIDE as usize;
const ACTIVE: usize = 176;
const READY: usize = 180;
const ERROR: usize = 200;
const ALARM_OFFSET: usize = 204;
const ERROR_TEXT: usize = 4096;

struct Window {
    store: wasmtime::Store<Events>,
    memory: wasmtime::Memory,
    result: Vec<Val>,
}

impl Window {
    fn word(&self, address: usize) -> i32 {
        let mut bytes = [0; 4];
        self.memory.read(&self.store, address, &mut bytes).unwrap();
        i32::from_le_bytes(bytes)
    }

    fn local(&self, local: u32) -> i32 {
        self.result[STATE.iter().position(|value| *value == local).unwrap()]
            .i32()
            .unwrap()
    }

    fn outcome(&self) -> i32 {
        self.result[HELPER_PARAMS].i32().unwrap()
    }

    fn assert_closed(&self, outcome: i32) {
        assert_eq!(self.outcome(), outcome);
        assert_eq!(
            self.store.data().live,
            if outcome == 4 {
                BTreeSet::from([7])
            } else {
                BTreeSet::new()
            },
            "enclosing timeout retains the scope alarm until scope unwind"
        );
        assert!(self.store.data().joined.is_empty());
        assert_eq!(self.store.data().closed_sets, [100]);
        for slot in [FIRST, SECOND] {
            assert_eq!(
                self.word(slot + DIRECT_PSPLIT_SLOT_SUBTASK_OFFSET as usize),
                0
            );
            assert_eq!(self.word(slot + ALARM_OFFSET), 0);
        }
    }
}

fn window(
    context: Context,
    ready: &[(i32, i32)],
    waiting: &[(i32, i32)],
    enclosing: bool,
    cancel_returns: i32,
) -> Window {
    // Two pending calls (1, 99), first call's deadline (2), per-call alarms
    // (5, 6), effective scope alarm (7), and optional enclosing deadline (8).
    // The first Agent timer is already armed; no clock race is simulated here.
    let mut live = BTreeSet::from([1, 99, 2, 5, 6, 7]);
    if enclosing {
        live.insert(8);
    }
    let (mut store, instance) = instantiate_events(
        context,
        true,
        Events {
            ready: ready.iter().copied().collect(),
            waiting: waiting.iter().copied().collect(),
            live,
            joined: BTreeMap::from([(1, 100), (99, 100), (2, 100)]),
            cancel_returns,
            root_cancel: matches!(context, Context::RootCancel),
            late_result: Some(FIRST + DIRECT_PSPLIT_SLOT_RESULT_OFFSET as usize),
            ..Default::default()
        },
    );
    let memory = instance.get_memory(&mut store, "test-memory").unwrap();
    memory
        .write(
            &mut store,
            FIRST,
            &vec![0; 2 * DIRECT_PSPLIT_SLOT_STRIDE as usize],
        )
        .unwrap();
    let fields = crate::direct_wasm::static_data::AGENT_TIMEOUT_FIELDS.concat();
    memory
        .write(&mut store, ERROR_TEXT, fields.as_bytes())
        .unwrap();
    for (slot, handle, alarm) in [(FIRST, 1i32, 5i32), (SECOND, 99, 6)] {
        for (offset, value) in [
            (DIRECT_PSPLIT_SLOT_SUBTASK_OFFSET as usize, handle),
            (ACTIVE, 1),
            (ERROR, ERROR_TEXT as i32),
            (ALARM_OFFSET, alarm),
        ] {
            memory
                .write(&mut store, slot + offset, &value.to_le_bytes())
                .unwrap();
        }
    }
    let mut params = vec![Val::I32(0); HELPER_PARAMS];
    for (local, value) in [
        (WINDOW_ACTIVE, 1),
        (WINDOW_BEGIN, FIRST as i32),
        (WINDOW_END, SECOND as i32 + DIRECT_PSPLIT_SLOT_STRIDE),
        (DIRECT_PSPLIT_WS_LOCAL, 100),
        (DEFER_BOUNDARY, 3),
        (parallel_deadline::ENABLED, 1),
        (parallel_deadline::OWNER, FIRST as i32),
        (parallel_deadline::TIMER_STATUS, 33),
        (super::super::super::deadline_scope::ALARM, 7),
        (DEADLINE_STATUS, if enclosing { 129 } else { 0 }),
    ] {
        params[STATE.iter().position(|entry| *entry == local).unwrap()] = Val::I32(value);
    }
    let mut result = vec![Val::I32(0); HELPER_PARAMS + 1];
    instance
        .get_func(&mut store, "test-window")
        .unwrap()
        .call(&mut store, &params, &mut result)
        .unwrap();
    Window {
        store,
        memory,
        result,
    }
}

#[test]
fn parallel_ready_completion_beats_own_deadline_in_every_event_order() {
    for ready in [
        [(1, RETURNED), (2, RETURNED), (99, RETURNED)],
        [(1, RETURNED), (99, RETURNED), (2, RETURNED)],
        [(2, RETURNED), (1, RETURNED), (99, RETURNED)],
        [(2, RETURNED), (99, RETURNED), (1, RETURNED)],
        [(99, RETURNED), (1, RETURNED), (2, RETURNED)],
        [(99, RETURNED), (2, RETURNED), (1, RETURNED)],
    ] {
        let w = window(Context::Callable, &ready, &[], false, CANCELLED);
        assert_eq!(w.outcome(), 0);
        assert_eq!(w.word(DIRECT_PSPLIT_EVENT_OFFSET as usize), 1);
        assert_eq!(w.store.data().live, BTreeSet::from([1, 99, 7]));
        assert!(w.store.data().joined.is_empty());
        for slot in [FIRST, SECOND] {
            assert_eq!(w.word(slot + READY), 1);
            assert_eq!(w.word(slot + ACTIVE), 0);
            assert_eq!(w.word(slot + DIRECT_PSPLIT_SLOT_RESULT_OFFSET as usize), 0);
        }
        assert!(!w.store.data().cancelled.contains(&1));
        assert!(!w.store.data().cancelled.contains(&99));
        assert_eq!(w.local(parallel_deadline::TIMER_STATUS), 0);
    }
}

#[test]
fn parallel_selected_timeout_survives_late_success_and_preserves_ready_peer() {
    for ready in [
        [(2, RETURNED), (99, RETURNED)],
        [(99, RETURNED), (2, RETURNED)],
    ] {
        for cancel_returns in [RETURNED, START_CANCELLED, CANCELLED] {
            let w = window(Context::Callable, &ready, &[], false, cancel_returns);
            assert_eq!(w.outcome(), 0);
            assert_eq!(w.store.data().live, BTreeSet::from([1, 99, 7]));
            assert!(w.store.data().joined.is_empty());
            assert_eq!(w.word(DIRECT_PSPLIT_EVENT_OFFSET as usize), 1);
            assert_eq!(w.word(FIRST + DIRECT_PSPLIT_SLOT_RESULT_OFFSET as usize), 1);
            let result = FIRST + DIRECT_PSPLIT_SLOT_RESULT_OFFSET as usize;
            let ptr = w.word(result + 8) as usize;
            let len = w.word(result + 12) as usize;
            let mut code = vec![0; len];
            w.memory.read(&w.store, ptr, &mut code).unwrap();
            assert_eq!(code, b"AGENT_TIMEOUT");
            assert_eq!(w.word(SECOND + READY), 1);
            assert_eq!(
                w.word(SECOND + DIRECT_PSPLIT_SLOT_RESULT_OFFSET as usize),
                0
            );
            assert!(w.store.data().cancelled.contains(&1));
            assert!(!w.store.data().cancelled.contains(&99));
        }
    }
}

#[test]
fn parallel_enclosing_timeout_owns_pending_window_even_with_ready_peer() {
    for ready in [
        [(8, RETURNED), (2, RETURNED), (99, RETURNED)],
        [(8, RETURNED), (99, RETURNED), (2, RETURNED)],
        [(2, RETURNED), (8, RETURNED), (99, RETURNED)],
        [(2, RETURNED), (99, RETURNED), (8, RETURNED)],
        [(99, RETURNED), (8, RETURNED), (2, RETURNED)],
        [(99, RETURNED), (2, RETURNED), (8, RETURNED)],
    ] {
        let w = window(Context::Callable, &ready, &[], true, CANCELLED);
        w.assert_closed(4);
        assert_eq!(w.local(DEFER_BOUNDARY), 2);
        assert_eq!(
            w.word(FIRST + DIRECT_PSPLIT_SLOT_RESULT_OFFSET as usize),
            0,
            "the Agent timeout must not replace the enclosing scope's outcome"
        );
    }
}

#[test]
fn parallel_root_and_parent_cancel_take_ownership_before_local_timeout() {
    for ready in [
        [(2, RETURNED), (99, RETURNED)],
        [(99, RETURNED), (2, RETURNED)],
    ] {
        let w = window(Context::RootCancel, &ready, &[], false, CANCELLED);
        w.assert_closed(2);
        assert_eq!(w.word(FIRST + DIRECT_PSPLIT_SLOT_RESULT_OFFSET as usize), 0);
        let cancellations = &w.store.data().cancelled;
        assert!(
            cancellations.iter().position(|v| *v == 7).unwrap()
                < cancellations.iter().position(|v| *v == 1).unwrap()
        );
    }
    let w = window(Context::Callable, &[], &[(0, 6)], true, CANCELLED);
    w.assert_closed(3);
    assert_eq!(w.store.data().cancelled.first(), Some(&7));
}
