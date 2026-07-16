//! Hand-emits the spike's three components with the PRODUCTION pins
//! (wasm-encoder / wit-parser / wit-component 0.247), using the same
//! pipeline as the direct emitter: raw core module + WIT metadata embed +
//! `wit_component::ComponentEncoder`.
//!
//! ABI selection rides on wit-component's LEGACY core-name manglings
//! (verified against wit-component 0.247 & 0.251 sources; identical strings):
//!
//! - async-LOWERED import:  module `"<iface>@<ver>"`, field `"[async-lower]<f>"`,
//!   core sig `(flat params…, retptr:i32) -> i32 status` (retptr only when the
//!   WIT function has a result; params collapse to one ptr above 4 flats).
//! - stackful async LIFT:   export `"[async-lift-stackful]<iface>@<ver>#<f>"`,
//!   core sig `(flat params…) -> ()`; results are delivered via the
//!   `[task-return]<f>` import from module `"[export]<iface>@<ver>"`.
//!   No callback export, no cabi_realloc needed for flat signatures.
//! - CM-async builtins:     module `"$root"`, fields `"[waitable-set-new]"`
//!   `() -> i32`, `"[waitable-set-wait]"` `(set,evptr) -> i32`,
//!   `"[waitable-set-drop]"` `(i32) -> ()`, `"[waitable-join]"`
//!   `(waitable,set) -> ()`, `"[subtask-drop]"` `(i32) -> ()`.
//! - sync lower / sync lift of async-TYPED functions use the PLAIN legacy
//!   names — the type stays async (blocking is legal), only the ABI is sync.
//!
//! Subtask status packing (low 4 bits): STARTING=0, STARTED=1, RETURNED=2;
//! upper 28 bits carry the subtask handle. `waitable-set.wait` writes
//! `{handle: u32, state: u32}` at evptr and returns the event code
//! (SUBTASK=1).

use anyhow::{Context, Result};
use wasm_encoder::{
    CodeSection, ExportKind, ExportSection, Function, FunctionSection, ImportSection,
    Instruction as I, MemArg, MemorySection, MemoryType, Module, TypeSection, ValType,
};
use wit_component::{ComponentEncoder, StringEncoding, embed_component_metadata};
use wit_parser::{Resolve, UnresolvedPackageGroup};

use crate::wit;

const RETURNED: i32 = 2;
/// Linear-memory layout (orchestrator): two u64 result buffers + event scratch.
const RETPTR_A: i32 = 16;
const RETPTR_B: i32 = 24;
const EVPTR: i32 = 32;

fn encode(mut core: Vec<u8>, wit_text: &str, world_name: &str) -> Result<Vec<u8>> {
    let mut resolve = Resolve::default();
    let group = UnresolvedPackageGroup::parse("spike.wit", wit_text).context("parse spike wit")?;
    let pkg = resolve.push_group(group).context("push wit group")?;
    let world = resolve
        .select_world(&[pkg], Some(world_name))
        .context("select world")?;
    embed_component_metadata(&mut core, &resolve, world, StringEncoding::UTF8)
        .context("embed metadata")?;
    ComponentEncoder::default()
        .module(&core)
        .context("encoder.module")?
        .validate(true)
        .encode()
        .context("component encode")
}

// ─── Plugins ─────────────────────────────────────────────────────────────────

/// Core module: import sync-lowered `sleep`, export sync-lifted async-typed
/// `run(ms) -> ms` that blocks in the host sleep.
fn plugin_core(iface: &str, func_export: &str) -> Vec<u8> {
    let mut types = TypeSection::new();
    // t0: sleep (i64) -> ()
    types.ty().function([ValType::I64], []);
    // t1: run (i64) -> (i64)
    types.ty().function([ValType::I64], [ValType::I64]);

    let mut imports = ImportSection::new();
    imports.import(iface, "sleep", wasm_encoder::EntityType::Function(0));

    let mut funcs = FunctionSection::new();
    funcs.function(1);

    let mut exports = ExportSection::new();
    exports.export(func_export, ExportKind::Func, 1);

    let mut body = Function::new([]);
    body.instruction(&I::LocalGet(0));
    body.instruction(&I::Call(0)); // sleep(ms) — blocks this task only
    body.instruction(&I::LocalGet(0));
    body.instruction(&I::End);
    let mut code = CodeSection::new();
    code.function(&body);

    let mut module = Module::new();
    module.section(&types);
    module.section(&imports);
    module.section(&funcs);
    module.section(&exports);
    module.section(&code);
    module.finish()
}

pub fn plugin_a() -> Result<Vec<u8>> {
    encode(
        plugin_core("demo:host/env@0.1.0", "demo:plugins/alpha@0.1.0#run"),
        wit::PLUGIN_A_WIT,
        "plugin-a",
    )
}

pub fn plugin_b() -> Result<Vec<u8>> {
    encode(
        plugin_core("demo:host/env@0.1.0", "demo:plugins/beta@0.1.0#run"),
        wit::PLUGIN_B_WIT,
        "plugin-b",
    )
}

// ─── Orchestrator ────────────────────────────────────────────────────────────

// Import function indices (order of the import section below).
const F_ALPHA: u32 = 0; // [async-lower]run  (i64, retptr) -> status
const F_BETA: u32 = 1;
const F_WS_NEW: u32 = 2; // () -> set
const F_WS_WAIT: u32 = 3; // (set, evptr) -> event
const F_WS_DROP: u32 = 4; // (set) -> ()
const F_JOIN: u32 = 5; // (waitable, set) -> ()
const F_SUBTASK_DROP: u32 = 6; // (subtask) -> ()
const F_TASK_RETURN_BOTH: u32 = 7; // (i64) -> ()
const F_TASK_RETURN_SEQ: u32 = 8;

// Locals (after the i64 `ms` param at index 0).
const L_MS: u32 = 0;
const L_WS: u32 = 1; // i32 waitable-set
const L_S: u32 = 2; // i32 launch status / scratch
const L_PENDING: u32 = 3; // i32 in-flight count

fn mem64() -> MemArg {
    MemArg {
        offset: 0,
        align: 3,
        memory_index: 0,
    }
}

fn mem32() -> MemArg {
    MemArg {
        offset: 0,
        align: 2,
        memory_index: 0,
    }
}

/// Launch one async-lowered call: `status = f(ms, retptr)`; if not eagerly
/// RETURNED, join the subtask into the set and bump `pending`.
fn emit_launch(body: &mut Function, func: u32, retptr: i32) {
    body.instruction(&I::LocalGet(L_MS));
    body.instruction(&I::I32Const(retptr));
    body.instruction(&I::Call(func));
    body.instruction(&I::LocalTee(L_S));
    body.instruction(&I::I32Const(0xF));
    body.instruction(&I::I32And);
    body.instruction(&I::I32Const(RETURNED));
    body.instruction(&I::I32Ne);
    body.instruction(&I::If(wasm_encoder::BlockType::Empty));
    {
        // waitable.join(status >> 4, ws)
        body.instruction(&I::LocalGet(L_S));
        body.instruction(&I::I32Const(4));
        body.instruction(&I::I32ShrU);
        body.instruction(&I::LocalGet(L_WS));
        body.instruction(&I::Call(F_JOIN));
        body.instruction(&I::LocalGet(L_PENDING));
        body.instruction(&I::I32Const(1));
        body.instruction(&I::I32Add);
        body.instruction(&I::LocalSet(L_PENDING));
    }
    body.instruction(&I::End);
}

/// Consume completion events until `pending == 0`. Blocks in
/// `waitable-set.wait` — legal because the export task is async-TYPED.
fn emit_drain(body: &mut Function) {
    body.instruction(&I::Block(wasm_encoder::BlockType::Empty)); // $done
    body.instruction(&I::Loop(wasm_encoder::BlockType::Empty)); // $poll
    {
        body.instruction(&I::LocalGet(L_PENDING));
        body.instruction(&I::I32Eqz);
        body.instruction(&I::BrIf(1)); // -> $done
        // event = waitable-set.wait(ws, EVPTR); ignore the code, dispatch on
        // the state written at EVPTR+4 (SUBTASK events: {handle, state}).
        body.instruction(&I::LocalGet(L_WS));
        body.instruction(&I::I32Const(EVPTR));
        body.instruction(&I::Call(F_WS_WAIT));
        body.instruction(&I::Drop);
        body.instruction(&I::I32Const(EVPTR + 4));
        body.instruction(&I::I32Load(mem32()));
        body.instruction(&I::I32Const(RETURNED));
        body.instruction(&I::I32Eq);
        body.instruction(&I::If(wasm_encoder::BlockType::Empty));
        {
            body.instruction(&I::I32Const(EVPTR));
            body.instruction(&I::I32Load(mem32()));
            body.instruction(&I::Call(F_SUBTASK_DROP));
            body.instruction(&I::LocalGet(L_PENDING));
            body.instruction(&I::I32Const(1));
            body.instruction(&I::I32Sub);
            body.instruction(&I::LocalSet(L_PENDING));
        }
        body.instruction(&I::End);
        body.instruction(&I::Br(0)); // -> $poll
    }
    body.instruction(&I::End); // loop
    body.instruction(&I::End); // block
}

/// `task.return(load64(RETPTR_A) + load64(RETPTR_B))`, then drop the set.
fn emit_finish(body: &mut Function, task_return: u32) {
    body.instruction(&I::LocalGet(L_WS));
    body.instruction(&I::Call(F_WS_DROP));
    body.instruction(&I::I32Const(RETPTR_A));
    body.instruction(&I::I64Load(mem64()));
    body.instruction(&I::I32Const(RETPTR_B));
    body.instruction(&I::I64Load(mem64()));
    body.instruction(&I::I64Add);
    body.instruction(&I::Call(task_return));
    body.instruction(&I::End);
}

fn orchestrator_core() -> Vec<u8> {
    let mut types = TypeSection::new();
    types.ty().function([ValType::I64, ValType::I32], [ValType::I32]); // 0: async-lowered run
    types.ty().function([], [ValType::I32]); // 1: ws-new
    types.ty().function([ValType::I32, ValType::I32], [ValType::I32]); // 2: ws-wait
    types.ty().function([ValType::I32], []); // 3: (i32) -> ()
    types.ty().function([ValType::I32, ValType::I32], []); // 4: join
    types.ty().function([ValType::I64], []); // 5: task-return / export shape

    let mut imports = ImportSection::new();
    let f = wasm_encoder::EntityType::Function;
    imports.import("demo:plugins/alpha@0.1.0", "[async-lower]run", f(0));
    imports.import("demo:plugins/beta@0.1.0", "[async-lower]run", f(0));
    imports.import("$root", "[waitable-set-new]", f(1));
    imports.import("$root", "[waitable-set-wait]", f(2));
    imports.import("$root", "[waitable-set-drop]", f(3));
    imports.import("$root", "[waitable-join]", f(4));
    imports.import("$root", "[subtask-drop]", f(3));
    imports.import("[export]demo:app/runner@0.1.0", "[task-return]run-both", f(5));
    imports.import("[export]demo:app/runner@0.1.0", "[task-return]run-seq", f(5));

    let mut funcs = FunctionSection::new();
    funcs.function(5); // run-both
    funcs.function(5); // run-seq

    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: 1,
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });

    let mut exports = ExportSection::new();
    exports.export("memory", ExportKind::Memory, 0);
    exports.export(
        "[async-lift-stackful]demo:app/runner@0.1.0#run-both",
        ExportKind::Func,
        9,
    );
    exports.export(
        "[async-lift-stackful]demo:app/runner@0.1.0#run-seq",
        ExportKind::Func,
        10,
    );

    // run-both: launch both, drain once — overlap.
    let mut both = Function::new([(3, ValType::I32)]);
    both.instruction(&I::Call(F_WS_NEW));
    both.instruction(&I::LocalSet(L_WS));
    emit_launch(&mut both, F_ALPHA, RETPTR_A);
    emit_launch(&mut both, F_BETA, RETPTR_B);
    emit_drain(&mut both);
    emit_finish(&mut both, F_TASK_RETURN_BOTH);

    // run-seq: launch, drain, launch, drain — the sequential baseline.
    let mut seq = Function::new([(3, ValType::I32)]);
    seq.instruction(&I::Call(F_WS_NEW));
    seq.instruction(&I::LocalSet(L_WS));
    emit_launch(&mut seq, F_ALPHA, RETPTR_A);
    emit_drain(&mut seq);
    emit_launch(&mut seq, F_BETA, RETPTR_B);
    emit_drain(&mut seq);
    emit_finish(&mut seq, F_TASK_RETURN_SEQ);

    let mut code = CodeSection::new();
    code.function(&both);
    code.function(&seq);

    let mut module = Module::new();
    module.section(&types);
    module.section(&imports);
    module.section(&funcs);
    module.section(&memories);
    module.section(&exports);
    module.section(&code);
    module.finish()
}

pub fn orchestrator() -> Result<Vec<u8>> {
    encode(orchestrator_core(), wit::ORCHESTRATOR_WIT, "orchestrator")
}
