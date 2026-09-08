//! Execute the actual emitted shared helper with deterministic canonical-event
//! fixtures. Real Component Model handle semantics have a separate integration
//! contract; this suite controls event ordering and verifies emitter decisions.
use super::super::*;
use super::*;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use wasm_encoder::{ExportKind, ExportSection, Module, RawSection};
use wasmtime::{Caller, ExternType, Linker, Val};

#[derive(Default)]
struct Events {
    ready: VecDeque<(i32, i32)>,
    waiting: VecDeque<(i32, i32)>,
    live: BTreeSet<i32>,
    joined: BTreeMap<i32, i32>,
    cancelled: Vec<i32>,
    dropped: Vec<i32>,
    closed_sets: Vec<i32>,
    cancel_returns: i32,
    root_cancel: bool,
}

#[derive(Clone, Copy)]
enum Context {
    Callable,
    Root,
    RootCancel,
}

fn emitted_helper(context: Context) -> Vec<u8> {
    let graph = serde_json::from_value(serde_json::json!({"durable":false,"entryPoint":"agent",
        "steps":{"agent":{"id":"agent","stepType":"Agent","agentId":"utils",
            "capabilityId":"random-double","maxRetries":0},
            "finish":{"id":"finish","stepType":"Finish"}},
        "executionPlan":[{"fromStep":"agent","toStep":"finish"}]}))
    .unwrap();
    let manifest = crate::direct_wasm::manifest::build_direct_workflow_manifest(&graph).unwrap();
    let config = DirectCoreConfig::new(&manifest, &manifest.to_canonical_json().unwrap(), false)
        .unwrap()
        .with_abi(if matches!(context, Context::Callable) {
            crate::direct_wasm::component::WorkflowAbi::AgentCapabilities
        } else {
            crate::direct_wasm::component::WorkflowAbi::InvokeHostImports
        })
        .with_omit_runtime(matches!(context, Context::Callable));
    let (resolve, world) =
        build_direct_component_resolve_with_agents(&manifest.feature_summary.agent_ids).unwrap();
    let bytes = emit_direct_core_module(&resolve, world, &config).unwrap();
    let mut imported = 0;
    let mut defined = 0;
    for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
        match payload.unwrap() {
            wasmparser::Payload::ImportSection(section) => {
                for import in section.into_imports() {
                    if matches!(import.unwrap().ty, wasmparser::TypeRef::Func(_)) {
                        imported += 1;
                    }
                }
            }
            wasmparser::Payload::FunctionSection(section) => defined = section.count(),
            _ => {}
        }
    }
    // Export helpers by their emitted registry positions, without assuming
    // Await/WindowWait remain the last functions when another helper is added.
    let helpers: Vec<_> = Helper::ALL
        .into_iter()
        .filter(|helper| !matches!(context, Context::Callable) || !helper.needs_runtime())
        .collect();
    let helper_index = |helper: Helper| {
        imported + defined - helpers.len() as u32
            + helpers
                .iter()
                .position(|candidate| *candidate as usize == helper as usize)
                .unwrap() as u32
    };
    let mut out = Module::new();
    for payload in wasmparser::Parser::new(0).parse_all(&bytes) {
        let payload = payload.unwrap();
        if let wasmparser::Payload::ExportSection(section) = payload {
            let mut exports = ExportSection::new();
            for export in section {
                let export = export.unwrap();
                let kind = match export.kind {
                    wasmparser::ExternalKind::Func => ExportKind::Func,
                    wasmparser::ExternalKind::Memory => ExportKind::Memory,
                    other => panic!("unexpected export {other:?}"),
                };
                exports.export(export.name, kind, export.index);
            }
            exports.export("test-await", ExportKind::Func, helper_index(Helper::Await));
            exports.export(
                "test-window",
                ExportKind::Func,
                helper_index(Helper::WindowWait),
            );
            exports.export("test-memory", ExportKind::Memory, 0);
            out.section(&exports);
        } else if let Some((id, range)) = payload.as_section() {
            out.section(&RawSection {
                id,
                data: &bytes[range],
            });
        }
    }
    out.finish()
}

fn memory(caller: &mut Caller<'_, Events>) -> wasmtime::Memory {
    caller
        .get_export("test-memory")
        .unwrap()
        .into_memory()
        .unwrap()
}

fn run(
    ready: &[(i32, i32)],
    waiting: &[(i32, i32)],
    target: i32,
    deadline: i32,
    cancel_returns: i32,
    expected_outcome: i32,
    expected_cancelled: &[i32],
) {
    run_in_context(
        Context::Callable,
        ready,
        waiting,
        target,
        deadline,
        cancel_returns,
        expected_outcome,
        expected_cancelled,
    );
}

#[allow(clippy::too_many_arguments)]
fn run_in_context(
    context: Context,
    ready: &[(i32, i32)],
    waiting: &[(i32, i32)],
    target: i32,
    deadline: i32,
    cancel_returns: i32,
    expected_outcome: i32,
    expected_cancelled: &[i32],
) {
    run_helper(
        context,
        false,
        ready,
        waiting,
        target,
        deadline,
        cancel_returns,
        expected_outcome,
        expected_cancelled,
    );
}

#[allow(clippy::too_many_arguments)]
fn run_helper(
    context: Context,
    window: bool,
    ready: &[(i32, i32)],
    waiting: &[(i32, i32)],
    target: i32,
    deadline: i32,
    cancel_returns: i32,
    expected_outcome: i32,
    expected_cancelled: &[i32],
) {
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::new(&engine, emitted_helper(context)).unwrap();
    let mut linker = Linker::<Events>::new(&engine);
    for import in module.imports() {
        let ExternType::Func(ty) = import.ty() else {
            panic!("nonfunction import");
        };
        let name = import.name().to_string();
        linker
            .func_new(
                import.module(),
                import.name(),
                ty,
                move |mut caller, args, results| {
                    let arg = |n: usize| args[n].i32().unwrap();
                    match name.as_str() {
                        "[waitable-set-new]" => results[0] = Val::I32(100),
                        "[waitable-join]" => {
                            assert!(
                                caller.data().live.contains(&arg(0)),
                                "join after resolution"
                            );
                            if arg(1) == 0 {
                                caller.data_mut().joined.remove(&arg(0));
                            } else {
                                assert!(caller.data_mut().joined.insert(arg(0), arg(1)).is_none());
                            }
                        }
                        "[waitable-set-poll]"
                        | "[cancellable][waitable-set-wait]"
                        | "[waitable-set-wait]" => {
                            assert_eq!(arg(0), 100);
                            let event = if name == "[waitable-set-poll]" {
                                caller.data_mut().ready.pop_front()
                            } else {
                                Some(
                                    caller
                                        .data_mut()
                                        .waiting
                                        .pop_front()
                                        .expect("unexpected blocking wait"),
                                )
                            };
                            if let Some((handle, state)) = event {
                                let mem = memory(&mut caller);
                                mem.write(&mut caller, arg(1) as usize, &handle.to_le_bytes())?;
                                mem.write(&mut caller, arg(1) as usize + 4, &state.to_le_bytes())?;
                                results[0] = Val::I32(if state == 6 { 6 } else { 1 });
                            } else {
                                results[0] = Val::I32(0);
                            }
                        }
                        "heartbeat" | "poll-signal" => {
                            let mem = memory(&mut caller);
                            mem.write(&mut caller, arg(0) as usize, &[0; 48])?;
                            if name == "poll-signal" && caller.data().root_cancel {
                                mem.write(&mut caller, 400, b"cancel")?;
                                mem.write(&mut caller, 416, b"cancel-test")?;
                                for (offset, value) in
                                    [(4, 1i32), (8, 400), (12, 6), (16, 416), (20, 11)]
                                {
                                    mem.write(
                                        &mut caller,
                                        arg(0) as usize + offset,
                                        &value.to_le_bytes(),
                                    )?;
                                }
                            }
                        }
                        "handle-checkpoint-signal" => {
                            let mem = memory(&mut caller);
                            mem.write(&mut caller, arg(4) as usize, &[0, 0, 0, 0, 1, 0, 0, 0])?;
                        }
                        "[async-lower]sleep" => {
                            assert!(caller.data_mut().live.insert(3));
                            results[0] = Val::I32(49);
                        }
                        "[subtask-cancel]" => {
                            assert!(caller.data().live.contains(&arg(0)));
                            assert!(
                                !caller.data().joined.contains_key(&arg(0)),
                                "cancel must detach first"
                            );
                            caller.data_mut().cancelled.push(arg(0));
                            results[0] = Val::I32(caller.data().cancel_returns);
                            // Even a normal return during cancellation cannot replace a
                            // deadline selected by the guest with a late success.
                            if caller.data().cancel_returns == RETURNED {
                                let mem = memory(&mut caller);
                                mem.write(&mut caller, 0, b"late")?;
                            }
                        }
                        "[subtask-drop]" => {
                            assert!(
                                caller.data_mut().live.remove(&arg(0)),
                                "double/unknown drop"
                            );
                            caller.data_mut().joined.remove(&arg(0));
                            caller.data_mut().dropped.push(arg(0));
                        }
                        "[waitable-set-drop]" => {
                            assert!(
                                !caller.data().joined.values().any(|set| *set == arg(0)),
                                "set dropped with a joined handle"
                            );
                            caller.data_mut().closed_sets.push(arg(0));
                        }
                        _ => return Err(wasmtime::Error::msg(format!("unexpected import {name}"))),
                    }
                    Ok(())
                },
            )
            .unwrap();
    }
    let live = [target, deadline]
        .into_iter()
        .filter(|s| *s >> 4 != 0)
        .map(|s| s >> 4)
        .chain([99])
        .collect();
    let mut store = wasmtime::Store::new(
        &engine,
        Events {
            ready: ready.iter().copied().collect(),
            waiting: waiting.iter().copied().collect(),
            live,
            joined: if window {
                BTreeMap::from([(1, 100), (99, 100)])
            } else {
                BTreeMap::from([(99, 200)])
            },
            cancel_returns,
            root_cancel: matches!(context, Context::RootCancel),
            ..Default::default()
        },
    );
    let instance = linker.instantiate(&mut store, &module).unwrap();
    let function = instance
        .get_func(
            &mut store,
            if window { "test-window" } else { "test-await" },
        )
        .unwrap();
    assert_eq!(function.ty(&store).params().len(), HELPER_PARAMS);
    let mut params = vec![Val::I32(0); HELPER_PARAMS];
    let mut set =
        |local, value| params[STATE.iter().position(|l| *l == local).unwrap()] = Val::I32(value);
    set(STATUS, target);
    set(DEADLINE_STATUS, deadline);
    set(WINDOW_ACTIVE, 1);
    set(WINDOW_BEGIN, 1024);
    set(
        WINDOW_END,
        1024 + super::super::DIRECT_PSPLIT_SLOT_STRIDE * if window { 2 } else { 1 },
    );
    set(DEFER_BOUNDARY, 3);
    set(
        super::super::DIRECT_PSPLIT_WS_LOCAL,
        if window { 100 } else { 200 },
    );
    let mem = instance.get_memory(&mut store, "test-memory").unwrap();
    mem.write(
        &mut store,
        1024 + super::super::DIRECT_PSPLIT_SLOT_SUBTASK_OFFSET as usize,
        &99i32.to_le_bytes(),
    )
    .unwrap();
    if window {
        mem.write(
            &mut store,
            1024 + super::super::DIRECT_PSPLIT_SLOT_SUBTASK_OFFSET as usize,
            &1i32.to_le_bytes(),
        )
        .unwrap();
        mem.write(
            &mut store,
            1024 + super::super::DIRECT_PSPLIT_SLOT_STRIDE as usize
                + super::super::DIRECT_PSPLIT_SLOT_SUBTASK_OFFSET as usize,
            &99i32.to_le_bytes(),
        )
        .unwrap();
    }
    let mut result = vec![Val::I32(0); HELPER_PARAMS + 1];
    function.call(&mut store, &params, &mut result).unwrap();
    assert_eq!(result[HELPER_PARAMS].i32(), Some(expected_outcome));
    assert_eq!(store.data().cancelled, expected_cancelled);
    assert_eq!(
        result[STATE.iter().position(|l| *l == DEADLINE_STATUS).unwrap()].i32(),
        Some(0)
    );
    if window {
        if expected_outcome == 0 {
            // An ordinary call is delivered intact; the entry drops it and
            // advances its slot, rather than counting a timer as a completion.
            assert_eq!(store.data().live, BTreeSet::from([1, 99]));
            assert_eq!(store.data().joined, BTreeMap::from([(1, 100), (99, 100)]));
            let mut event = [0; 8];
            mem.read(&store, DIRECT_PSPLIT_EVENT_OFFSET as usize, &mut event)
                .unwrap();
            assert_eq!(event, [1, 0, 0, 0, 2, 0, 0, 0]);
            assert_eq!(
                result[STATE.iter().position(|l| *l == DEFER_BOUNDARY).unwrap()].i32(),
                Some(3)
            );
        } else {
            assert!(store.data().live.is_empty());
            assert!(store.data().joined.is_empty());
            assert_eq!(store.data().closed_sets, vec![100]);
            if expected_outcome == 4 {
                assert_eq!(
                    result[STATE.iter().position(|l| *l == DEFER_BOUNDARY).unwrap()].i32(),
                    Some(2)
                );
            }
        }
        return;
    }
    if matches!(expected_outcome, 2 | 3) {
        assert!(store.data().live.is_empty());
        assert!(store.data().joined.is_empty());
    } else {
        assert_eq!(
            store.data().live,
            BTreeSet::from([99]),
            "unrelated sibling must survive"
        );
        assert_eq!(store.data().joined, BTreeMap::from([(99, 200)]));
        assert_eq!(
            result[STATE.iter().position(|l| *l == WINDOW_ACTIVE).unwrap()].i32(),
            Some(1)
        );
    }
}

#[test]
fn emitted_deadline_completion_ties_drain_in_both_orders() {
    for ready in [
        vec![(1, 2), (2, 2)],
        vec![(2, 2), (1, 2)],
        vec![(2, 1), (1, 1), (2, 2), (1, 2)],
    ] {
        run(&ready, &[], 17, 33, CANCELLED, 0, &[]);
    }
}
#[test]
fn emitted_deadline_cancels_only_owned_call_and_keeps_selected_reason() {
    for returned in [RETURNED, START_CANCELLED, CANCELLED] {
        run(&[(2, 2)], &[], 17, 33, returned, 4, &[1]);
    }
}
#[test]
fn emitted_deadline_success_resolves_pending_timer() {
    run(&[(1, 2)], &[], 17, 33, CANCELLED, 0, &[2]);
    run(&[], &[], RETURNED, 33, CANCELLED, 0, &[2]);
}
#[test]
fn emitted_deadline_already_due_still_accepts_ready_completion() {
    run(&[], &[], 17, RETURNED, CANCELLED, 4, &[1]);
    run(&[(1, 2)], &[], 17, RETURNED, CANCELLED, 0, &[]);
}
#[test]
fn emitted_deadline_parent_cancel_resolves_deadline_and_sibling() {
    run(&[], &[(0, 6)], 17, 33, CANCELLED, 3, &[2, 1, 99]);
}
#[test]
fn emitted_unbounded_wait_retains_existing_event_path() {
    run(&[], &[(1, 1), (1, 2)], 17, 0, CANCELLED, 0, &[]);
}

#[test]
fn emitted_deadline_root_cancel_wins_before_timeout_recovery() {
    run_in_context(
        Context::RootCancel,
        &[(2, 2)],
        &[],
        17,
        33,
        CANCELLED,
        2,
        &[1, 99],
    );
}

#[test]
fn emitted_deadline_root_poll_timer_is_resolved_without_touching_sibling() {
    run_in_context(Context::Root, &[], &[(2, 2)], 17, 33, CANCELLED, 4, &[1, 3]);
}

#[test]
fn emitted_window_deadline_delivers_ready_calls_before_expiry() {
    for (ready, cancelled) in [
        (vec![(1, 2), (2, 2)], vec![2]),
        (vec![(2, 2), (1, 2)], vec![]),
        (vec![(2, 1), (1, 1), (2, 2), (1, 2)], vec![]),
        (vec![(1, 2)], vec![2]),
    ] {
        run_helper(
            Context::Callable,
            true,
            &ready,
            &[],
            17,
            33,
            CANCELLED,
            0,
            &cancelled,
        );
    }
    run_helper(
        Context::Callable,
        true,
        &[(1, 2)],
        &[],
        17,
        RETURNED,
        CANCELLED,
        0,
        &[],
    );
}

#[test]
fn emitted_window_deadline_resolves_every_owned_call_and_releases_deferral() {
    for returned in [RETURNED, START_CANCELLED, CANCELLED] {
        run_helper(
            Context::Callable,
            true,
            &[(2, 2)],
            &[],
            17,
            33,
            returned,
            4,
            &[1, 99],
        );
    }
    run_helper(
        Context::Callable,
        true,
        &[],
        &[],
        17,
        RETURNED,
        CANCELLED,
        4,
        &[1, 99],
    );
}

#[test]
fn emitted_window_deadline_preserves_root_and_parent_cancel_priority() {
    run_helper(
        Context::RootCancel,
        true,
        &[(2, 2)],
        &[],
        17,
        33,
        CANCELLED,
        2,
        &[1, 99],
    );
    run_helper(
        Context::Callable,
        true,
        &[],
        &[(0, 6)],
        17,
        33,
        CANCELLED,
        3,
        &[2, 1, 99],
    );
}

#[test]
fn emitted_window_deadline_resolves_the_lifecycle_poll_timer() {
    run_helper(
        Context::Root,
        true,
        &[],
        &[(2, 2)],
        17,
        33,
        CANCELLED,
        4,
        &[1, 99, 3],
    );
    // A completed lifecycle timer is internal too, and never escapes as a
    // completed Agent call or decrements the caller's pending count.
    run_helper(
        Context::Root,
        true,
        &[],
        &[(3, 2), (2, 2)],
        17,
        33,
        CANCELLED,
        4,
        &[1, 99, 3],
    );
}
