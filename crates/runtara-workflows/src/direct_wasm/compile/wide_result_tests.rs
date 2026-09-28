// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! A `result<u64, string>` (like `runtime.now-ms`) or `result<option<u64>,
//! string>` keeps its error string at +8/+12, not at +4/+8 where every
//! shared error reader looks. `emit_call_wide_result` moves it.

use super::abi::{RETPTR_WIDE_ERR_PTR_OFFSET, emit_call_wide_result, load_retptr_list};
use super::*;
use wasm_encoder::{
    CodeSection, ExportKind, ExportSection, FunctionSection, MemArg, MemorySection, MemoryType,
    Module, TypeSection, ValType,
};

const PADDING: i32 = 0x0bad_f00d;
const ERR_PTR: i32 = 1234;
const ERR_LEN: i32 = 56;
const OK_VALUE: i64 = 0x1122_3344_5566_7788;

/// Run a fake host call that writes `result<u64, string>` with `tag`, then
/// read it back the way the emitter does: `(error ptr, error len, u64 at +8)`.
fn run(tag: u8, normalize: bool) -> (i32, i32, i64) {
    let mem = |offset| MemArg {
        offset,
        align: 2,
        memory_index: 0,
    };
    let mut module = Module::new();
    let mut types = TypeSection::new();
    types.ty().function([], []);
    types
        .ty()
        .function([], [ValType::I32, ValType::I32, ValType::I64]);
    module.section(&types);
    let mut functions = FunctionSection::new();
    functions.function(0);
    functions.function(1);
    module.section(&functions);
    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: 1,
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    module.section(&memories);
    let mut exports = ExportSection::new();
    exports.export("run", ExportKind::Func, 1);
    module.section(&exports);

    let mut code = CodeSection::new();
    // The canonical lowering of the host result into the retptr area.
    let mut host = WasmFunction::new([]);
    host.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    host.instruction(&Instruction::I32Const(i32::from(tag)));
    host.instruction(&Instruction::I32Store8(MemArg {
        offset: 0,
        align: 0,
        memory_index: 0,
    }));
    host.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
    host.instruction(&Instruction::I32Const(PADDING));
    host.instruction(&Instruction::I32Store(mem(4)));
    if tag == 0 {
        host.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
        host.instruction(&Instruction::I64Const(OK_VALUE));
        host.instruction(&Instruction::I64Store(MemArg {
            offset: 8,
            align: 3,
            memory_index: 0,
        }));
    } else {
        for (offset, value) in [(8, ERR_PTR), (12, ERR_LEN)] {
            host.instruction(&Instruction::I32Const(DIRECT_RUN_RETPTR_OFFSET));
            host.instruction(&Instruction::I32Const(value));
            host.instruction(&Instruction::I32Store(mem(offset)));
        }
    }
    host.instruction(&Instruction::End);
    code.function(&host);

    let mut body = WasmFunction::new([(2, ValType::I32)]);
    if normalize {
        emit_call_wide_result(&mut body, 0);
    } else {
        body.instruction(&Instruction::Call(0));
    }
    load_retptr_list(&mut body, 0, 1);
    body.instruction(&Instruction::LocalGet(0));
    body.instruction(&Instruction::LocalGet(1));
    abi::push_retptr_i64_load(&mut body, DIRECT_RET_U64_OK_OFFSET);
    body.instruction(&Instruction::End);
    code.function(&body);
    module.section(&code);

    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::new(&engine, module.finish()).unwrap();
    let mut store = wasmtime::Store::new(&engine, ());
    let instance = wasmtime::Instance::new(&mut store, &module, &[]).unwrap();
    instance
        .get_typed_func::<(), (i32, i32, i64)>(&mut store, "run")
        .unwrap()
        .call(&mut store, ())
        .unwrap()
}

#[test]
fn a_wide_result_error_reaches_the_shared_readers_intact() {
    // Before the fix the readers took the padding as the pointer and the
    // pointer as the length.
    let (ptr, len, _) = run(1, false);
    assert_eq!((ptr, len), (PADDING, ERR_PTR));

    let (ptr, len, _) = run(1, true);
    assert_eq!((ptr, len), (ERR_PTR, ERR_LEN));
}

#[test]
fn a_wide_result_ok_value_is_untouched() {
    let (_, _, value) = run(0, true);
    assert_eq!(value, OK_VALUE);
}

/// The offsets come from the WIT: 8-aligned ok arms move the error string to
/// +8, while the readers' +4 is right for the 4-aligned ones.
#[test]
fn wide_result_offsets_match_the_wit_layout() {
    use wit_parser::{Int, Resolve, SizeAlign, Type, TypeDefKind};
    let resolve: Resolve = runtara_wit::resolve().unwrap();
    let runtime = runtara_package(&resolve, runtara_wit::workflow::PACKAGE);
    let stdlib = runtara_package(&resolve, runtara_wit::stdlib::PACKAGE);
    let mut sizes = SizeAlign::default();
    sizes.fill(&resolve);
    let err_offset = |package, interface: &str, function: &str| {
        let interface = &resolve.interfaces[resolve.packages[package].interfaces[interface]];
        let Some(Type::Id(result)) = interface.functions[function].result else {
            panic!("{function} returns a result");
        };
        let TypeDefKind::Result(result) = &resolve.types[result].kind else {
            panic!("{function} returns a result");
        };
        sizes
            .payload_offset(Int::U8, [result.ok.as_ref(), result.err.as_ref()])
            .size_wasm32() as u64
    };
    for (package, interface, function) in [
        (runtime, "runtime", "now-ms"),
        (stdlib, "json", "delay-duration-ms"),
        (stdlib, "json", "wait-timeout-ms"),
        (stdlib, "json", "wait-poll-interval-ms"),
        (stdlib, "json", "retry-delay-ms"),
        (stdlib, "json", "workflow-error-retry-after-ms"),
        (stdlib, "json", "agent-retry-delay-ms"),
    ] {
        assert_eq!(
            err_offset(package, interface, function),
            RETPTR_WIDE_ERR_PTR_OFFSET,
            "{function}"
        );
    }
    assert_eq!(err_offset(runtime, "runtime", "load-input"), 4);
}

fn runtara_package(resolve: &wit_parser::Resolve, name: &str) -> wit_parser::PackageId {
    resolve
        .packages
        .iter()
        .find(|(_, package)| package.name.to_string() == name)
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("{name} is in the resolve"))
}
