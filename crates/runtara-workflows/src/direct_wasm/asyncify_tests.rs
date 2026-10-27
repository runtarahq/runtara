// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The pass on hand-written modules in the emitter's shapes: a body run
//! through a driver that unwinds at every wait and is called back with the
//! event must return what the same body returns when every wait answers at
//! once, with the same side effects, and leave the buffer empty.

use super::*;

/// Globals: 0 state, 1 data, 2 event, 3 pending, 4 entries, 5 log count.
/// Functions: 0 wait (the leaf), 1 body, 2 helper, 3 log, 4 run, 5 resume.
const BODY: &str = r#"
  (func $body (export "body") (param $n i32) (result i32)
    (local $i i32) (local $acc i32) (local $e i32) (local $wide i64)
    ;; Runs once per call of the body, never again while rewinding.
    (global.set $entries (i32.add (global.get $entries) (i32.const 1)))
    (local.set $wide (i64.const 0x100000000))
    (block $out
      (loop $top
        (br_if $out (i32.ge_u (local.get $i) (local.get $n)))
        (local.set $e (call $wait (local.get $i)))
        (if (i32.and (local.get $e) (i32.const 1))
          (then
            (local.set $acc (i32.add (local.get $acc) (local.get $e)))
            (local.set $e (call $helper (i32.add (local.get $i) (i32.const 100))))
            (local.set $acc (i32.add (local.get $acc) (local.get $e))))
          (else
            (local.set $acc (i32.sub (local.get $acc) (i32.const 1)))
            (block $skip
              (br_if $skip (i32.eqz (local.get $i)))
              (drop (call $wait (i32.add (local.get $i) (i32.const 1000)))))))
        (call $log (local.get $acc))
        (local.set $wide (i64.add (local.get $wide) (i64.const 1)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $top)))
    (i32.add (local.get $acc) (i32.wrap_i64 (i64.shr_u (local.get $wide) (i64.const 32)))))

  (func $helper (param $x i32) (result i32) (local $r i32)
    (call $log (i32.const 7000))
    (local.set $r (call $wait (local.get $x)))
    (call $log (local.get $r))
    (local.get $r))

  (func $log (param $v i32)
    (i32.store (i32.add (i32.const 4096) (i32.shl (global.get $count) (i32.const 2)))
      (local.get $v))
    (global.set $count (i32.add (global.get $count) (i32.const 1))))
"#;

const GLOBALS: &str = r#"
  (memory (export "memory") 1)
  (global $state (export "state") (mut i32) (i32.const 0))
  (global $data (mut i32) (i32.const 0))
  (global $event (mut i32) (i32.const 0))
  (global $pending (export "pending") (mut i32) (i32.const 0))
  (global $entries (export "entries") (mut i32) (i32.const 0))
  (global $count (export "count") (mut i32) (i32.const 0))
"#;

/// The event a wait on `x` is answered with.
fn answer(x: i32) -> i32 {
    x * 10 + (x % 2)
}

fn synchronous(body: &str) -> String {
    format!(
        r#"(module {GLOBALS}
          (func $wait (param $x i32) (result i32)
            (i32.add (i32.mul (local.get $x) (i32.const 10)) (i32.rem_u (local.get $x) (i32.const 2))))
          {body})"#
    )
}

fn asynchronous(body: &str) -> String {
    format!(
        r#"(module {GLOBALS}
          (func $wait (param $x i32) (result i32)
            (if (i32.eq (global.get $state) (i32.const 2))
              (then (global.set $state (i32.const 0)) (return (global.get $event))))
            (global.set $pending (local.get $x))
            (global.set $state (i32.const 1))
            (i32.const 0))
          {body}
          (func $run (export "run") (param $n i32) (result i32) (local $r i32)
            (i32.store (i32.const 1024) (i32.const 1032))
            (i32.store (i32.const 1028) (i32.const 4000))
            (global.set $data (i32.const 1024))
            (local.set $r (call $body (local.get $n)))
            (if (i32.eq (global.get $state) (i32.const 1))
              (then (global.set $state (i32.const 0)) (return (i32.const -1))))
            (local.get $r))
          (func $resume (export "resume") (param $event i32) (result i32) (local $r i32)
            (global.set $event (local.get $event))
            (global.set $state (i32.const 2))
            (local.set $r (call $body (i32.const 0)))
            (if (i32.eq (global.get $state) (i32.const 1))
              (then (global.set $state (i32.const 0)) (return (i32.const -1))))
            (local.get $r)))"#
    )
}

fn config() -> AsyncifyConfig {
    AsyncifyConfig {
        leaves: [0].into(),
        drivers: [4, 5].into(),
        state_global: 0,
        data_global: 1,
    }
}

struct Run {
    result: i32,
    entries: i32,
    log: Vec<i32>,
    waits: usize,
    cursor: i32,
}

fn instance(wasm: &[u8]) -> (wasmtime::Store<()>, wasmtime::Instance) {
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::new(&engine, wasm).expect("module compiles");
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = wasmtime::Instance::new(&mut store, &module, &[]).expect("instantiates");
    (store, instance)
}

fn read(store: &mut wasmtime::Store<()>, instance: &wasmtime::Instance, global: &str) -> i32 {
    instance
        .get_global(&mut *store, global)
        .expect("exported global")
        .get(&mut *store)
        .i32()
        .expect("i32 global")
}

fn log(store: &mut wasmtime::Store<()>, instance: &wasmtime::Instance) -> Vec<i32> {
    let count = read(store, instance, "count") as usize;
    let memory = instance.get_memory(&mut *store, "memory").expect("memory");
    let bytes = &memory.data(&*store)[4096..4096 + count * 4];
    bytes
        .chunks(4)
        .map(|word| i32::from_le_bytes(word.try_into().unwrap()))
        .collect()
}

fn run_synchronous(body: &str, n: i32) -> Run {
    let wasm = wat::parse_str(synchronous(body)).expect("wat");
    let (mut store, instance) = instance(&wasm);
    let body = instance
        .get_typed_func::<i32, i32>(&mut store, "body")
        .expect("body");
    let result = body.call(&mut store, n).expect("runs");
    Run {
        result,
        entries: read(&mut store, &instance, "entries"),
        log: log(&mut store, &instance),
        waits: 0,
        cursor: 1032,
    }
}

fn run_unwinding(body: &str, n: i32) -> Run {
    let wasm = wat::parse_str(asynchronous(body)).expect("wat");
    let (wasm, instrumented) = asyncify(&wasm, &config()).expect("instruments");
    assert!(instrumented.contains(&1), "the body is instrumented");
    wasmparser::Validator::new_with_features(WasmFeatures::all())
        .validate_all(&wasm)
        .expect("the instrumented module validates");
    let (mut store, instance) = instance(&wasm);
    let run = instance
        .get_typed_func::<i32, i32>(&mut store, "run")
        .expect("run");
    let resume = instance
        .get_typed_func::<i32, i32>(&mut store, "resume")
        .expect("resume");
    let mut waits = 0;
    let mut result = run.call(&mut store, n).expect("runs");
    while result == -1 {
        waits += 1;
        assert!(waits < 1000, "the body keeps unwinding");
        let pending = read(&mut store, &instance, "pending");
        result = resume.call(&mut store, answer(pending)).expect("resumes");
    }
    let memory = instance.get_memory(&mut store, "memory").expect("memory");
    let cursor = i32::from_le_bytes(memory.data(&store)[1024..1028].try_into().unwrap());
    Run {
        result,
        entries: read(&mut store, &instance, "entries"),
        log: log(&mut store, &instance),
        waits,
        cursor,
    }
}

/// Run `body` both ways for each `n` and compare.
fn same_both_ways(body: &str, ns: &[i32]) {
    for &n in ns {
        let expected = run_synchronous(body, n);
        let actual = run_unwinding(body, n);
        assert_eq!(actual.result, expected.result, "result for n={n}");
        assert_eq!(actual.log, expected.log, "side effects for n={n}");
        assert_eq!(
            actual.entries, expected.entries,
            "rewinding does not run the body's entry again (n={n})"
        );
        assert_eq!(actual.cursor, 1032, "every frame was popped (n={n})");
        assert!(actual.waits >= 1, "it unwound (n={n})");
    }
}

#[test]
fn an_unwound_body_returns_what_a_synchronous_one_does() {
    same_both_ways(BODY, &[1, 2, 5, 9]);
}

/// Calls whose results feed expressions, whose arguments are calls, which
/// decide an `if`, and values left on the stack across a block that waits.
#[test]
fn values_on_the_operand_stack_survive_an_unwind() {
    let body = r#"(func $body (export "body") (param $n i32) (result i32) (local $e i32)
             (global.set $entries (i32.add (global.get $entries) (i32.const 1)))
             ;; A result used in an expression, and an argument that calls.
             (local.set $e (i32.add (call $wait (call $helper (local.get $n))) (i32.const 3)))
             (call $log (local.get $e))
             ;; A wait deciding an `if`.
             (if (i32.and (call $wait (i32.add (local.get $n) (i32.const 1))) (i32.const 1))
               (then (call $log (i32.const 11)))
               (else (call $log (i32.const 22))))
             ;; Values kept on the stack across a block that waits.
             (local.get $n)
             (i32.const 5)
             (block
               (local.set $e (call $wait (i32.const 4)))
               (call $log (local.get $e)))
             (i32.add)
             (local.get $e)
             (i32.add))
           (func $helper (param $x i32) (result i32) (local $r i32)
             (call $log (i32.const 7000))
             (local.set $r (call $wait (local.get $x)))
             (call $log (local.get $r))
             (local.get $r))
           (func $log (param $v i32)
             (i32.store (i32.add (i32.const 4096) (i32.shl (global.get $count) (i32.const 2)))
               (local.get $v))
             (global.set $count (i32.add (global.get $count) (i32.const 1))))"#;
    same_both_ways(body, &[0, 1, 2, 7]);
}

fn refusal(body: &str) -> String {
    let wasm = wat::parse_str(asynchronous(body)).expect("wat");
    asyncify(&wasm, &config())
        .expect_err("the shape is refused")
        .0
}

/// The rest of `BODY`'s functions, for bodies that replace `$body`.
const SUPPORT: &str = r#"
  (func $helper (param $x i32) (result i32) (local.get $x))
  (func $log (param $v i32))
"#;

#[test]
fn a_block_with_a_value_around_a_wait_is_refused() {
    let message = refusal(&format!(
        r#"(func $body (param $n i32) (result i32) (local $e i32)
             (block (result i32)
               (local.set $e (call $wait (local.get $n)))
               (local.get $e))) {SUPPORT}"#
    ));
    assert!(message.contains("parameters or has results"), "{message}");
}

#[test]
fn dead_code_after_a_branch_is_left_alone() {
    let body = format!(
        r#"(func $body (param $n i32) (result i32) (local $e i32)
             (block $out
               (local.set $e (call $wait (local.get $n)))
               (br $out)
               (drop (i32.add (call $wait (i32.const 9)) (i32.const 1))))
             (local.get $e)) {SUPPORT}"#
    );
    let wasm = wat::parse_str(asynchronous(&body)).expect("wat");
    let (wasm, _) = asyncify(&wasm, &config()).expect("dead shapes are not checked");
    let (mut store, instance) = instance(&wasm);
    let run = instance
        .get_typed_func::<i32, i32>(&mut store, "run")
        .unwrap();
    let resume = instance
        .get_typed_func::<i32, i32>(&mut store, "resume")
        .unwrap();
    assert_eq!(run.call(&mut store, 4).unwrap(), -1);
    assert_eq!(resume.call(&mut store, 41).unwrap(), 41);
}
