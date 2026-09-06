//! Guest bridge from the Agent ABI to owned execution resources. Graph control
//! stays in the parent. V1 identifies live calls; V3 accepts compiler-qualified
//! call-site and attempt identity through a private interface.
use super::*;
use std::collections::BTreeMap;
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, EntityType, ExportKind, ExportSection,
    FunctionSection, GlobalSection, GlobalType, ImportSection, MemArg, MemorySection, MemoryType,
    Module, TypeSection, ValType,
};
use wit_parser::{
    ManglingAndAbi, ResourceIntrinsic, WasmExport, WasmExportKind, WasmImport, WorldItem,
};

fn emit(body: &mut WasmFunction, instructions: impl IntoIterator<Item = Instruction<'static>>) {
    for instruction in instructions {
        body.instruction(&instruction);
    }
}
fn mem(offset: u64) -> MemArg {
    MemArg {
        offset,
        align: 0,
        memory_index: 0,
    }
}
fn address(body: &mut WasmFunction, frame: u32, offset: i32) {
    emit(
        body,
        [
            Instruction::LocalGet(frame),
            Instruction::I32Const(offset),
            Instruction::I32Add,
        ],
    );
}
fn check_result(body: &mut WasmFunction, frame: u32, offset: u64) {
    emit(
        body,
        [
            Instruction::LocalGet(frame),
            Instruction::I32Load8U(mem(offset)),
            Instruction::If(BlockType::Empty),
            Instruction::Unreachable,
            Instruction::End,
        ],
    );
}

pub(super) fn emit_adapter(agent: &str, binding: &str) -> Result<Vec<u8>, DirectCompileError> {
    emit_adapter_configured(agent, binding, false)
}

pub(super) fn emit_adapter_configured(
    agent: &str,
    binding: &str,
    scoped: bool,
) -> Result<Vec<u8>, DirectCompileError> {
    use runtara_workflow_wit::{EXECUTION_INTERFACE_NAME, EXECUTION_WIT};
    let mut resolve = Resolve::default();
    for (name, source) in [
        ("abi.wit", ABI_WIT),
        ("lifecycle.wit", LIFECYCLE_WIT),
        ("execution.wit", EXECUTION_WIT),
        ("agent-types.wit", AGENT_TYPES_WIT),
    ] {
        resolve.push_str(name, source).map_err(component_error)?;
    }
    resolve
        .push_str("agent.wit", &agent_wit_package_configured(agent, scoped))
        .map_err(component_error)?;
    let interface = if scoped {
        "scoped-capabilities-v3"
    } else {
        "capabilities"
    };
    let package = resolve.push_str("adapter.wit", &format!(
        "package runtara:isolated-adapter; world adapter {{ import {EXECUTION_INTERFACE_NAME}; export runtara:agent-{agent}/{interface}@{AGENT_WIT_VERSION}; }}"
    )).map_err(component_error)?;
    let world = resolve
        .select_world(&[package], Some("adapter"))
        .map_err(component_error)?;
    let mangling = ManglingAndAbi::Standard32;
    let mut types = TypeSection::new();
    let mut type_count = 0;
    let mut imports = ImportSection::new();
    let mut calls = BTreeMap::new();
    for (key, item) in &resolve.worlds[world].imports {
        let WorldItem::Interface { id, .. } = item else {
            continue;
        };
        // `use` brings shared type interfaces into Resolve too. Import only
        // execution functions, never the lifecycle invoke used by wake types.
        if resolve.interfaces[*id].name.as_deref() != Some("tasks") {
            continue;
        }
        for function in resolve.interfaces[*id].functions.values() {
            let signature = resolve.wasm_signature(mangling.import_variant(), function);
            let ty = abi::push_core_type(
                &mut types,
                &mut type_count,
                &signature.params,
                &signature.results,
            );
            let (module, name) = resolve.wasm_import_name(
                mangling,
                WasmImport::Func {
                    interface: Some(key),
                    func: function,
                },
            );
            calls.insert(function.name.clone(), imports.len());
            imports.import(&module, &name, EntityType::Function(ty));
        }
        if let Some(resource) = resolve.interfaces[*id].types.get("task") {
            let ty = type_count;
            type_count += 1;
            types.ty().function([ValType::I32], []);
            let (module, name) = resolve.wasm_import_name(
                mangling,
                WasmImport::ResourceIntrinsic {
                    interface: Some(key),
                    resource: *resource,
                    intrinsic: ResourceIntrinsic::ImportedDrop,
                },
            );
            calls.insert("drop".into(), imports.len());
            imports.import(&module, &name, EntityType::Function(ty));
        }
    }
    let mut data_bytes = Vec::new();
    let mut text = |value: &str| -> Result<(i32, i32), DirectCompileError> {
        let offset = i32::try_from(1024 + data_bytes.len()).map_err(component_error)?;
        let len = i32::try_from(value.len()).map_err(component_error)?;
        data_bytes.extend_from_slice(value.as_bytes());
        Ok((offset, len))
    };
    let binding = text(binding)?;
    let cancelled = [
        text("CANCELLED")?,
        text("isolated invocation cancelled")?,
        text("cancellation")?,
        text("error")?,
    ];
    let timed_out = [
        text("TIMEOUT")?,
        text("isolated invocation timed out")?,
        text("timeout")?,
        text("error")?,
    ];
    let heap = i32::try_from((1024 + data_bytes.len() + 4095) & !4095).map_err(component_error)?;
    let mut functions = FunctionSection::new();
    let mut exports = ExportSection::new();
    let mut code = CodeSection::new();
    let realloc_index = imports.len();
    functions.function(type_count);
    type_count += 1;
    types.ty().function([ValType::I32; 4], [ValType::I32]);
    exports.export(
        &resolve.wasm_export_name(mangling, WasmExport::Realloc),
        ExportKind::Func,
        realloc_index,
    );
    code.function(&realloc());
    for (key, item) in &resolve.worlds[world].exports {
        let WorldItem::Interface { id, .. } = item else {
            continue;
        };
        for function in resolve.interfaces[*id].functions.values() {
            let signature = resolve.wasm_signature(mangling.export_variant(), function);
            let ty = abi::push_core_type(
                &mut types,
                &mut type_count,
                &signature.params,
                &signature.results,
            );
            let index = imports.len() + functions.len();
            functions.function(ty);
            exports.export(
                &resolve.wasm_export_name(
                    mangling,
                    WasmExport::Func {
                        interface: Some(key),
                        func: function,
                        kind: WasmExportKind::Normal,
                    },
                ),
                ExportKind::Func,
                index,
            );
            let frame = if scoped { 9 } else { 4 };
            let handle = frame + 1;
            let mut body = WasmFunction::new([(if scoped { 3 } else { 2 }, ValType::I32)]);
            emit(
                &mut body,
                [
                    Instruction::GlobalGet(2),
                    Instruction::I32Const(1),
                    Instruction::I32Add,
                    Instruction::GlobalSet(2),
                    Instruction::I32Const(0),
                    Instruction::I32Const(0),
                    Instruction::I32Const(8),
                    Instruction::I32Const(384),
                    Instruction::Call(realloc_index),
                    Instruction::LocalSet(frame),
                ],
            );
            if scoped {
                emit_context_path(&mut body, frame + 2, realloc_index);
            } else {
                emit(
                    &mut body,
                    [
                        Instruction::GlobalGet(1),
                        Instruction::I64Const(-1),
                        Instruction::I64Eq,
                        Instruction::If(BlockType::Empty),
                        Instruction::Unreachable,
                        Instruction::End,
                        Instruction::GlobalGet(1),
                        Instruction::I64Const(1),
                        Instruction::I64Add,
                        Instruction::GlobalSet(1),
                    ],
                );
            }
            emit(
                &mut body,
                [
                    Instruction::I32Const(binding.0),
                    Instruction::I32Const(binding.1),
                    Instruction::I32Const(0),
                    Instruction::LocalGet(0),
                    Instruction::LocalGet(1),
                    Instruction::LocalGet(2),
                    Instruction::LocalGet(3),
                ],
            );
            if scoped {
                emit(
                    &mut body,
                    [
                        Instruction::LocalGet(frame + 2),
                        Instruction::LocalGet(5),
                        Instruction::I32Const(18),
                        Instruction::I32Add,
                        Instruction::LocalGet(8),
                    ],
                );
            } else {
                emit(
                    &mut body,
                    [
                        Instruction::I32Const(binding.0),
                        Instruction::I32Const(binding.1),
                        Instruction::GlobalGet(1),
                    ],
                );
            }
            emit(
                &mut body,
                [
                    Instruction::LocalGet(frame),
                    Instruction::Call(calls["start"]),
                ],
            );
            check_result(&mut body, frame, 0);
            emit(
                &mut body,
                [
                    Instruction::LocalGet(frame),
                    Instruction::I32Load(mem(4)),
                    Instruction::LocalSet(handle),
                    Instruction::LocalGet(handle),
                ],
            );
            address(&mut body, frame, 32);
            emit(&mut body, [Instruction::Call(calls["join"])]);
            check_result(&mut body, frame, 32);
            address(&mut body, frame, 192);
            emit(
                &mut body,
                [
                    Instruction::I32Const(0),
                    Instruction::I32Const(80),
                    Instruction::MemoryFill(0),
                ],
            );
            // join.ok.completed bytes -> Agent result.ok.
            emit(
                &mut body,
                [
                    Instruction::LocalGet(frame),
                    Instruction::I32Load8U(mem(40)),
                    Instruction::I32Eqz,
                    Instruction::If(BlockType::Empty),
                ],
            );
            address(&mut body, frame, 200);
            address(&mut body, frame, 48);
            emit(
                &mut body,
                [
                    Instruction::I32Const(8),
                    Instruction::MemoryCopy {
                        src_mem: 0,
                        dst_mem: 0,
                    },
                    Instruction::Else,
                    Instruction::LocalGet(frame),
                    Instruction::I32Load8U(mem(40)),
                    Instruction::I32Const(1),
                    Instruction::I32Eq,
                    Instruction::If(BlockType::Empty),
                ],
            );
            address(&mut body, frame, 192);
            emit(
                &mut body,
                [Instruction::I32Const(1), Instruction::I32Store(mem(0))],
            );
            address(&mut body, frame, 200);
            address(&mut body, frame, 48);
            emit(
                &mut body,
                [
                    Instruction::I32Const(72),
                    Instruction::MemoryCopy {
                        src_mem: 0,
                        dst_mem: 0,
                    },
                    Instruction::Else,
                ],
            );
            for (tag, fields) in [(3, cancelled), (4, timed_out)] {
                emit(
                    &mut body,
                    [
                        Instruction::LocalGet(frame),
                        Instruction::I32Load8U(mem(40)),
                        Instruction::I32Const(tag),
                        Instruction::I32Eq,
                        Instruction::If(BlockType::Empty),
                    ],
                );
                address(&mut body, frame, 192);
                emit(
                    &mut body,
                    [Instruction::I32Const(1), Instruction::I32Store(mem(0))],
                );
                for (n, (ptr, len)) in fields.into_iter().enumerate() {
                    address(&mut body, frame, 200 + n as i32 * 8);
                    emit(
                        &mut body,
                        [Instruction::I32Const(ptr), Instruction::I32Store(mem(0))],
                    );
                    address(&mut body, frame, 204 + n as i32 * 8);
                    emit(
                        &mut body,
                        [Instruction::I32Const(len), Instruction::I32Store(mem(0))],
                    );
                }
                emit(&mut body, [Instruction::Else]);
            }
            // Capabilities cannot return lifecycle wakes. Child traps and host
            // errors preserve fatal trap behavior rather than becoming retries.
            emit(
                &mut body,
                [
                    Instruction::Unreachable,
                    Instruction::End,
                    Instruction::End,
                    Instruction::End,
                    Instruction::End,
                    Instruction::LocalGet(handle),
                ],
            );
            address(&mut body, frame, 128);
            emit(&mut body, [Instruction::Call(calls["release"])]);
            check_result(&mut body, frame, 128);
            emit(
                &mut body,
                [
                    Instruction::LocalGet(handle),
                    Instruction::Call(calls["drop"]),
                ],
            );
            address(&mut body, frame, 192);
            emit(&mut body, [Instruction::End]);
            code.function(&body);
            let post_ty = abi::push_core_type(&mut types, &mut type_count, &signature.results, &[]);
            let post_index = imports.len() + functions.len();
            functions.function(post_ty);
            exports.export(
                &resolve.wasm_export_name(
                    mangling,
                    WasmExport::Func {
                        interface: Some(key),
                        func: function,
                        kind: WasmExportKind::PostReturn,
                    },
                ),
                ExportKind::Func,
                post_index,
            );
            let mut post = WasmFunction::new([]);
            emit(
                &mut post,
                [
                    Instruction::GlobalGet(2),
                    Instruction::I32Const(1),
                    Instruction::I32Sub,
                    Instruction::GlobalSet(2),
                    Instruction::GlobalGet(2),
                    Instruction::I32Eqz,
                    Instruction::If(BlockType::Empty),
                    Instruction::I32Const(heap),
                    Instruction::GlobalSet(0),
                    Instruction::End,
                    Instruction::End,
                ],
            );
            code.function(&post);
        }
    }
    let mut memory = MemorySection::new();
    memory.memory(MemoryType {
        minimum: (heap as u64).div_ceil(65536).max(1),
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    exports.export(
        &resolve.wasm_export_name(mangling, WasmExport::Memory),
        ExportKind::Memory,
        0,
    );
    let mut globals = GlobalSection::new();
    globals.global(
        GlobalType {
            val_type: ValType::I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(heap),
    );
    globals.global(
        GlobalType {
            val_type: ValType::I64,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i64_const(0),
    );
    globals.global(
        GlobalType {
            val_type: ValType::I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(0),
    );
    let mut data = DataSection::new();
    data.active(0, &ConstExpr::i32_const(1024), data_bytes);
    let mut module = Module::new();
    module
        .section(&types)
        .section(&imports)
        .section(&functions)
        .section(&memory)
        .section(&globals)
        .section(&exports)
        .section(&code)
        .section(&data);
    let mut module = module.finish();
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8)
        .map_err(component_error)?;
    ComponentEncoder::default()
        .module(&module)
        .map_err(component_error)?
        .validate(true)
        .encode()
        .map_err(component_error)
}

fn realloc() -> WasmFunction {
    // i64 arithmetic prevents wraparound; alignment and failed grow are checked.
    let mut body = WasmFunction::new([(2, ValType::I64)]);
    emit(
        &mut body,
        [
            Instruction::LocalGet(3),
            Instruction::I32Eqz,
            Instruction::If(BlockType::Empty),
            Instruction::I32Const(0),
            Instruction::Return,
            Instruction::End,
            Instruction::LocalGet(2),
            Instruction::I32Eqz,
            Instruction::If(BlockType::Empty),
            Instruction::Unreachable,
            Instruction::End,
            Instruction::GlobalGet(0),
            Instruction::I64ExtendI32U,
            Instruction::LocalGet(2),
            Instruction::I64ExtendI32U,
            Instruction::I64Const(1),
            Instruction::I64Sub,
            Instruction::I64Add,
            Instruction::I64Const(0),
            Instruction::LocalGet(2),
            Instruction::I64ExtendI32U,
            Instruction::I64Sub,
            Instruction::I64And,
            Instruction::LocalSet(4),
            Instruction::LocalGet(4),
            Instruction::LocalGet(3),
            Instruction::I64ExtendI32U,
            Instruction::I64Add,
            Instruction::LocalSet(5),
            Instruction::LocalGet(5),
            Instruction::I64Const(u32::MAX as i64),
            Instruction::I64GtU,
            Instruction::If(BlockType::Empty),
            Instruction::Unreachable,
            Instruction::End,
            Instruction::LocalGet(5),
            Instruction::I64Const(65535),
            Instruction::I64Add,
            Instruction::I64Const(16),
            Instruction::I64ShrU,
            Instruction::MemorySize(0),
            Instruction::I64ExtendI32U,
            Instruction::I64GtU,
            Instruction::If(BlockType::Empty),
            Instruction::LocalGet(5),
            Instruction::I64Const(65535),
            Instruction::I64Add,
            Instruction::I64Const(16),
            Instruction::I64ShrU,
            Instruction::I32WrapI64,
            Instruction::MemorySize(0),
            Instruction::I32Sub,
            Instruction::MemoryGrow(0),
            Instruction::I32Const(-1),
            Instruction::I32Eq,
            Instruction::If(BlockType::Empty),
            Instruction::Unreachable,
            Instruction::End,
            Instruction::End,
            Instruction::LocalGet(5),
            Instruction::I32WrapI64,
            Instruction::GlobalSet(0),
            Instruction::LocalGet(0),
            Instruction::If(BlockType::Empty),
            Instruction::LocalGet(4),
            Instruction::I32WrapI64,
            Instruction::LocalGet(0),
            Instruction::LocalGet(1),
            Instruction::LocalGet(3),
            Instruction::LocalGet(1),
            Instruction::LocalGet(3),
            Instruction::I32LtU,
            Instruction::Select,
            Instruction::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            },
            Instruction::End,
            Instruction::LocalGet(4),
            Instruction::I32WrapI64,
            Instruction::End,
        ],
    );
    body
}

/// The compiler supplies the durable Agent key plus a qualified call-site token
/// and activation index. Fixed-width base-16 (a–p) keeps the suffix injective without JSON
/// parsing or host policy. The attempt stays a separate u64 in the task context.
fn emit_context_path(body: &mut WasmFunction, path: u32, realloc: u32) {
    emit(
        body,
        [
            Instruction::LocalGet(5),
            Instruction::I32Const(11),
            Instruction::I32LtU,
            Instruction::If(BlockType::Empty),
            Instruction::Unreachable,
            Instruction::End,
        ],
    );
    for (offset, byte) in b"runtara:v2:".iter().enumerate() {
        emit(
            body,
            [
                Instruction::LocalGet(4),
                Instruction::I32Load8U(mem(offset as u64)),
                Instruction::I32Const(i32::from(*byte)),
                Instruction::I32Ne,
                Instruction::If(BlockType::Empty),
                Instruction::Unreachable,
                Instruction::End,
            ],
        );
    }
    emit(
        body,
        [
            Instruction::LocalGet(5),
            Instruction::I32Const(-19),
            Instruction::I32GtU,
            Instruction::If(BlockType::Empty),
            Instruction::Unreachable,
            Instruction::End,
            Instruction::I32Const(0),
            Instruction::I32Const(0),
            Instruction::I32Const(1),
            Instruction::LocalGet(5),
            Instruction::I32Const(18),
            Instruction::I32Add,
            Instruction::Call(realloc),
            Instruction::LocalSet(path),
            Instruction::LocalGet(path),
            Instruction::LocalGet(4),
            Instruction::LocalGet(5),
            Instruction::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            },
        ],
    );
    // Only the copied invocation address changes version. The parent's durable
    // checkpoint key and input buffer remain byte-for-byte unchanged.
    emit(
        body,
        [
            Instruction::LocalGet(path),
            Instruction::I32Const(i32::from(b'3')),
            Instruction::I32Store8(mem(9)),
        ],
    );
    for (field, offset) in [(6, 0), (7, 9)] {
        emit(
            body,
            [
                Instruction::LocalGet(path),
                Instruction::LocalGet(5),
                Instruction::I32Add,
                Instruction::I32Const(58),
                Instruction::I32Store8(mem(offset)),
            ],
        );
        for digit in 0..8 {
            emit(
                body,
                [
                    Instruction::LocalGet(path),
                    Instruction::LocalGet(5),
                    Instruction::I32Add,
                    Instruction::LocalGet(field),
                    Instruction::I32Const((7 - digit) * 4),
                    Instruction::I32ShrU,
                    Instruction::I32Const(15),
                    Instruction::I32And,
                    // Encode 0..15 as 'a'..'p': equally injective, no branch/table.
                    Instruction::I32Const(97),
                    Instruction::I32Add,
                    Instruction::I32Store8(mem(offset + 1 + digit as u64)),
                ],
            );
        }
    }
}
