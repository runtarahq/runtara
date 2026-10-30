// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The callback lift of a published workflow-agent (SYN-770).
//!
//! A composed workflow-agent sees its caller's cancel only as an event of the
//! component-model event loop, so its `invoke` is lifted `async` with a
//! `callback` instead of synchronously. The emitted body is unchanged: it
//! still calls the cooperative wait (`waitable-set.wait`) and yield
//! (`thread.yield`) where it waits. This pass, run on the finished core
//! module:
//!
//! 1. instruments the body with [`crate::direct_wasm::asyncify`], the two
//!    builtins being the leaves;
//! 2. points every call to them at two leaf functions of its own: outside a
//!    rewind they record what to wait for and start unwinding; while
//!    rewinding they stop it and answer with the event the loop delivered;
//! 3. adds the drivers, exported as `[async-lift]…#invoke` and
//!    `[callback][async-lift]…#invoke`. Both run the body; when it unwound
//!    they return `WAIT | set << 4` (or `YIELD`), and when it returned they
//!    pass its result to `task.return` and return `EXIT`. The callback first
//!    stores the event and starts the rewind.
//!
//! `task.return` takes the yield builtin's import slot, so no function index
//! moves; the wait builtin keeps its slot without `cancellable`, unused. The
//! unwind buffer sits where the heap started, which now starts after it.

use std::collections::BTreeSet;

use wasm_encoder::reencode::{Reencode, utils};
use wasm_encoder::{
    BlockType, ConstExpr, Encode, EntityType, Function, GlobalType, Instruction, MemArg, ValType,
};
use wit_parser::abi::{AbiVariant, WasmType};
use wit_parser::{
    Function as WitFunction, LiftLowerAbi, Mangling, ManglingAndAbi, Resolve, WasmExport,
    WasmExportKind, WorldKey,
};

use super::{DirectCompileError, component_error};
use crate::direct_wasm::asyncify::{AsyncifyConfig, REWINDING, UNWINDING, asyncify};

const WAIT: &str = "[cancellable][waitable-set-wait]";
const YIELD: &str = "[cancellable][thread-yield]";

/// Bytes reserved for the unwind buffer: its `[cursor, end]` header, then
/// frames. The deepest stack is the entry, a few helpers and a loop's
/// scope; each frame holds every local of its function.
const BUFFER_BYTES: i32 = 64 * 1024;

/// Callback codes.
const EXIT: i32 = 0;
const YIELD_CODE: i32 = 1;
const WAIT_CODE: i32 = 2;
/// `EVENT_CANCELLED`.
const CANCELLED: i32 = 6;

/// What the module looks like before the lift.
struct Layout {
    types: u32,
    imported_functions: u32,
    imported_globals: u32,
    defined_functions: u32,
    defined_globals: u32,
    wait: Option<u32>,
    yield_: u32,
    entry: u32,
    entry_params: Vec<wasmparser::ValType>,
    heap_base: i32,
}

/// Lift `invoke` of `interface` with a callback.
pub(super) fn lift_with_callback(
    core: &[u8],
    resolve: &Resolve,
    interface: &WorldKey,
    invoke: &WitFunction,
) -> Result<Vec<u8>, DirectCompileError> {
    let sync_name = resolve.wasm_export_name(
        ManglingAndAbi::Standard32,
        WasmExport::Func {
            interface: Some(interface),
            func: invoke,
            kind: WasmExportKind::Normal,
        },
    );
    let post_return_name = resolve.wasm_export_name(
        ManglingAndAbi::Standard32,
        WasmExport::Func {
            interface: Some(interface),
            func: invoke,
            kind: WasmExportKind::PostReturn,
        },
    );
    let layout = read_layout(core, &sync_name)?;

    // Globals the pass and the leaves share, after the module's own.
    let first = layout.imported_globals + layout.defined_globals;
    let globals = Globals {
        state: first,
        data: first + 1,
        mode: first + 2,
        pending: first + 3,
        event: first + 4,
        payload: first + 5,
        payload2: first + 6,
    };
    let leaves: BTreeSet<u32> = layout.wait.into_iter().chain([layout.yield_]).collect();
    let (instrumented, _) = asyncify(
        core,
        &AsyncifyConfig {
            leaves,
            drivers: BTreeSet::new(),
            state_global: globals.state,
            data_global: globals.data,
        },
    )
    .map_err(component_error)?;

    let async_export = |kind| {
        resolve.wasm_export_name(
            ManglingAndAbi::Legacy(LiftLowerAbi::AsyncCallback),
            WasmExport::Func {
                interface: Some(interface),
                func: invoke,
                kind,
            },
        )
    };
    let lift_signature = resolve.wasm_signature(AbiVariant::GuestExportAsync, invoke);
    let lift_params: Vec<ValType> = lift_signature.params.iter().map(core_type).collect();
    let entry_params: Vec<ValType> = layout
        .entry_params
        .iter()
        .map(|ty| match ty {
            wasmparser::ValType::I32 => Ok(ValType::I32),
            wasmparser::ValType::I64 => Ok(ValType::I64),
            other => Err(component_error(format!(
                "callback lift: entry parameter of type {other:?}"
            ))),
        })
        .collect::<Result<_, _>>()?;
    if lift_params != entry_params {
        return Err(component_error(
            "callback lift: the async lift's parameters differ from the entry's",
        ));
    }
    let (task_return_module, task_return_name, task_return) =
        invoke.task_return_import(resolve, Some(interface), Mangling::Legacy);
    if task_return.params != [WasmType::Pointer] {
        return Err(component_error(format!(
            "callback lift: task.return takes {:?}, not the result's address",
            task_return.params
        )));
    }

    let defined = layout.imported_functions + layout.defined_functions;
    let mut lift = Lift {
        layout: &layout,
        globals,
        leaf_wait: defined,
        leaf_yield: defined + 1,
        driver: defined + 2,
        callback: defined + 3,
        types: TypesAdded {
            wait: layout.types,
            yield_: layout.types + 1,
            task_return: layout.types + 2,
            entry: layout.types + 3,
            callback: layout.types + 4,
        },
        lift_params,
        task_return_module,
        task_return_name,
        sync_name,
        post_return_name,
        async_name: async_export(WasmExportKind::Normal),
        callback_name: async_export(WasmExportKind::Callback),
        function_imports: 0,
        global_index: 0,
    };
    let mut out = wasm_encoder::Module::new();
    lift.parse_core_module(&mut out, wasmparser::Parser::new(0), &instrumented)
        .map_err(|e| component_error(format!("callback lift: {e:?}")))?;
    Ok(out.finish())
}

fn core_type(ty: &WasmType) -> ValType {
    match ty {
        WasmType::I64 => ValType::I64,
        WasmType::F32 => ValType::F32,
        WasmType::F64 => ValType::F64,
        // Pointers, lengths and pointer-or-scalar words are wasm32 i32s.
        WasmType::I32 | WasmType::Pointer | WasmType::Length | WasmType::PointerOrI64 => {
            ValType::I32
        }
    }
}

fn read_layout(core: &[u8], sync_name: &str) -> Result<Layout, DirectCompileError> {
    let wrap = |e: wasmparser::BinaryReaderError| component_error(e.to_string());
    let mut types = Vec::new();
    let mut function_types = Vec::new();
    let mut layout = Layout {
        types: 0,
        imported_functions: 0,
        imported_globals: 0,
        defined_functions: 0,
        defined_globals: 0,
        wait: None,
        yield_: u32::MAX,
        entry: u32::MAX,
        entry_params: Vec::new(),
        heap_base: -1,
    };
    for payload in wasmparser::Parser::new(0).parse_all(core) {
        match payload.map_err(wrap)? {
            wasmparser::Payload::TypeSection(reader) => {
                for ty in reader.into_iter_err_on_gc_types() {
                    types.push(ty.map_err(wrap)?.params().to_vec());
                }
                layout.types = types.len() as u32;
            }
            wasmparser::Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.map_err(wrap)?;
                    match import.ty {
                        wasmparser::TypeRef::Func(ty) => {
                            if import.name == WAIT {
                                layout.wait = Some(layout.imported_functions);
                            } else if import.name == YIELD {
                                layout.yield_ = layout.imported_functions;
                            }
                            function_types.push(ty);
                            layout.imported_functions += 1;
                        }
                        wasmparser::TypeRef::Global(_) => layout.imported_globals += 1,
                        _ => {}
                    }
                }
            }
            wasmparser::Payload::FunctionSection(reader) => {
                for ty in reader {
                    function_types.push(ty.map_err(wrap)?);
                    layout.defined_functions += 1;
                }
            }
            wasmparser::Payload::GlobalSection(reader) => {
                for (n, global) in reader.into_iter().enumerate() {
                    let global = global.map_err(wrap)?;
                    if n == 0 {
                        let mut ops = global.init_expr.get_operators_reader();
                        if let Ok(wasmparser::Operator::I32Const { value }) = ops.read() {
                            layout.heap_base = value;
                        }
                    }
                    layout.defined_globals += 1;
                }
            }
            wasmparser::Payload::ExportSection(reader) => {
                for export in reader {
                    let export = export.map_err(wrap)?;
                    if export.name == sync_name {
                        layout.entry = export.index;
                    }
                }
            }
            _ => {}
        }
    }
    if layout.yield_ == u32::MAX {
        return Err(component_error("callback lift: no yield builtin import"));
    }
    if layout.entry == u32::MAX {
        return Err(component_error(format!(
            "callback lift: no `{sync_name}` export"
        )));
    }
    if layout.heap_base < 0 {
        return Err(component_error(
            "callback lift: the heap pointer is not the first global",
        ));
    }
    let entry_type = function_types[layout.entry as usize];
    layout.entry_params = types[entry_type as usize].clone();
    Ok(layout)
}

#[derive(Clone, Copy)]
struct Globals {
    state: u32,
    /// The unwind buffer's header address.
    data: u32,
    /// What the leaf that unwound waits for: 0 a set, 1 a yield.
    mode: u32,
    /// The set a wait unwound on.
    pending: u32,
    /// The event the loop delivered, and its two payload words.
    event: u32,
    payload: u32,
    payload2: u32,
}

#[derive(Clone, Copy)]
struct TypesAdded {
    wait: u32,
    yield_: u32,
    task_return: u32,
    entry: u32,
    callback: u32,
}

struct Lift<'l> {
    layout: &'l Layout,
    globals: Globals,
    leaf_wait: u32,
    leaf_yield: u32,
    driver: u32,
    callback: u32,
    types: TypesAdded,
    lift_params: Vec<ValType>,
    task_return_module: String,
    task_return_name: String,
    sync_name: String,
    post_return_name: String,
    async_name: String,
    callback_name: String,
    function_imports: u32,
    global_index: u32,
}

impl Reencode for Lift<'_> {
    type Error = std::convert::Infallible;

    fn function_index(&mut self, func: u32) -> Result<u32, wasm_encoder::reencode::Error> {
        Ok(if Some(func) == self.layout.wait {
            self.leaf_wait
        } else if func == self.layout.yield_ {
            self.leaf_yield
        } else {
            func
        })
    }

    fn parse_type_section(
        &mut self,
        types: &mut wasm_encoder::TypeSection,
        section: wasmparser::TypeSectionReader<'_>,
    ) -> Result<(), wasm_encoder::reencode::Error> {
        utils::parse_type_section(self, types, section)?;
        types
            .ty()
            .function([ValType::I32, ValType::I32], [ValType::I32]);
        types.ty().function([], [ValType::I32]);
        types.ty().function([ValType::I32], []);
        types
            .ty()
            .function(self.lift_params.clone(), [ValType::I32]);
        types
            .ty()
            .function([ValType::I32, ValType::I32, ValType::I32], [ValType::I32]);
        Ok(())
    }

    // One import at a time: the default encodes import groups whole.
    fn parse_import_section(
        &mut self,
        imports: &mut wasm_encoder::ImportSection,
        section: wasmparser::ImportSectionReader<'_>,
    ) -> Result<(), wasm_encoder::reencode::Error> {
        for import in section.into_imports() {
            let import = import?;
            let entity = self.entity_type(import.ty)?;
            if let wasmparser::TypeRef::Func(_) = import.ty {
                let index = self.function_imports;
                self.function_imports += 1;
                if index == self.layout.yield_ {
                    imports.import(
                        &self.task_return_module,
                        &self.task_return_name,
                        EntityType::Function(self.types.task_return),
                    );
                    continue;
                }
                if Some(index) == self.layout.wait {
                    imports.import(import.module, "[waitable-set-wait]", entity);
                    continue;
                }
            }
            imports.import(import.module, import.name, entity);
        }
        Ok(())
    }

    fn parse_function_section(
        &mut self,
        functions: &mut wasm_encoder::FunctionSection,
        section: wasmparser::FunctionSectionReader<'_>,
    ) -> Result<(), wasm_encoder::reencode::Error> {
        utils::parse_function_section(self, functions, section)?;
        functions.function(self.types.wait);
        functions.function(self.types.yield_);
        functions.function(self.types.entry);
        functions.function(self.types.callback);
        Ok(())
    }

    fn parse_global_section(
        &mut self,
        globals: &mut wasm_encoder::GlobalSection,
        section: wasmparser::GlobalSectionReader<'_>,
    ) -> Result<(), wasm_encoder::reencode::Error> {
        for global in section {
            let global = global?;
            let ty = self.global_type(global.ty)?;
            // The heap starts after the unwind buffer.
            let init = if self.global_index == 0 {
                ConstExpr::i32_const(self.layout.heap_base + BUFFER_BYTES)
            } else {
                self.const_expr(global.init_expr)?
            };
            globals.global(ty, &init);
            self.global_index += 1;
        }
        for _ in 0..7 {
            globals.global(
                GlobalType {
                    val_type: ValType::I32,
                    mutable: true,
                    shared: false,
                },
                &ConstExpr::i32_const(0),
            );
        }
        Ok(())
    }

    fn parse_memory_section(
        &mut self,
        memories: &mut wasm_encoder::MemorySection,
        section: wasmparser::MemorySectionReader<'_>,
    ) -> Result<(), wasm_encoder::reencode::Error> {
        for memory in section {
            let mut ty = self.memory_type(memory?)?;
            let needed = (self.layout.heap_base as u64 + BUFFER_BYTES as u64).div_ceil(65_536);
            ty.minimum = ty.minimum.max(needed);
            memories.memory(ty);
        }
        Ok(())
    }

    fn parse_export_section(
        &mut self,
        exports: &mut wasm_encoder::ExportSection,
        section: wasmparser::ExportSectionReader<'_>,
    ) -> Result<(), wasm_encoder::reencode::Error> {
        for export in section {
            let export = export?;
            if export.name == self.sync_name || export.name == self.post_return_name {
                continue;
            }
            self.parse_export(exports, export)?;
        }
        exports.export(
            &self.async_name,
            wasm_encoder::ExportKind::Func,
            self.driver,
        );
        exports.export(
            &self.callback_name,
            wasm_encoder::ExportKind::Func,
            self.callback,
        );
        Ok(())
    }

    fn parse_code_section(
        &mut self,
        code: &mut wasm_encoder::CodeSection,
        section: wasmparser::CodeSectionReader<'_>,
    ) -> Result<(), wasm_encoder::reencode::Error> {
        utils::parse_code_section(self, code, section)?;
        code.function(&self.leaf_wait_body());
        code.function(&self.leaf_yield_body());
        code.function(&self.driver_body());
        code.function(&self.callback_body());
        Ok(())
    }
}

fn mem32(offset: u64) -> MemArg {
    MemArg {
        offset,
        align: 2,
        memory_index: 0,
    }
}

/// A function body built from instructions.
struct Body {
    bytes: Vec<u8>,
}

impl Body {
    fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    fn i(&mut self, instruction: Instruction<'_>) -> &mut Self {
        instruction.encode(&mut self.bytes);
        self
    }

    fn finish(self, locals: Vec<(u32, ValType)>) -> Function {
        let mut function = Function::new(locals);
        function.raw(self.bytes);
        function.instruction(&Instruction::End);
        function
    }
}

impl Lift<'_> {
    fn buffer(&self) -> i32 {
        self.layout.heap_base
    }

    /// `wait(set, payload-address) -> event`, in place of the builtin.
    fn leaf_wait_body(&self) -> Function {
        let g = self.globals;
        let mut b = Body::new();
        b.i(Instruction::GlobalGet(g.state))
            .i(Instruction::I32Const(REWINDING))
            .i(Instruction::I32Eq)
            .i(Instruction::If(BlockType::Empty))
            .i(Instruction::I32Const(0))
            .i(Instruction::GlobalSet(g.state))
            .i(Instruction::LocalGet(1))
            .i(Instruction::GlobalGet(g.payload))
            .i(Instruction::I32Store(mem32(0)))
            .i(Instruction::LocalGet(1))
            .i(Instruction::GlobalGet(g.payload2))
            .i(Instruction::I32Store(mem32(4)))
            .i(Instruction::GlobalGet(g.event))
            .i(Instruction::Return)
            .i(Instruction::End);
        b.i(Instruction::LocalGet(0))
            .i(Instruction::GlobalSet(g.pending))
            .i(Instruction::I32Const(0))
            .i(Instruction::GlobalSet(g.mode))
            .i(Instruction::I32Const(UNWINDING))
            .i(Instruction::GlobalSet(g.state))
            .i(Instruction::I32Const(0));
        b.finish(vec![])
    }

    /// `yield() -> cancelled`, in place of the builtin.
    fn leaf_yield_body(&self) -> Function {
        let g = self.globals;
        let mut b = Body::new();
        b.i(Instruction::GlobalGet(g.state))
            .i(Instruction::I32Const(REWINDING))
            .i(Instruction::I32Eq)
            .i(Instruction::If(BlockType::Empty))
            .i(Instruction::I32Const(0))
            .i(Instruction::GlobalSet(g.state))
            .i(Instruction::GlobalGet(g.event))
            .i(Instruction::I32Const(CANCELLED))
            .i(Instruction::I32Eq)
            .i(Instruction::Return)
            .i(Instruction::End);
        b.i(Instruction::I32Const(1))
            .i(Instruction::GlobalSet(g.mode))
            .i(Instruction::I32Const(UNWINDING))
            .i(Instruction::GlobalSet(g.state))
            .i(Instruction::I32Const(0));
        b.finish(vec![])
    }

    /// After the body: the callback code for an unwind, else `task.return`
    /// of its result (in `result`) and `EXIT`.
    fn finish_run(&self, b: &mut Body, result: u32) {
        let g = self.globals;
        b.i(Instruction::GlobalGet(g.state))
            .i(Instruction::I32Const(UNWINDING))
            .i(Instruction::I32Eq)
            .i(Instruction::If(BlockType::Empty))
            .i(Instruction::I32Const(0))
            .i(Instruction::GlobalSet(g.state))
            .i(Instruction::GlobalGet(g.mode))
            .i(Instruction::If(BlockType::Empty))
            .i(Instruction::I32Const(YIELD_CODE))
            .i(Instruction::Return)
            .i(Instruction::End)
            .i(Instruction::GlobalGet(g.pending))
            .i(Instruction::I32Const(4))
            .i(Instruction::I32Shl)
            .i(Instruction::I32Const(WAIT_CODE))
            .i(Instruction::I32Or)
            .i(Instruction::Return)
            .i(Instruction::End);
        b.i(Instruction::LocalGet(result))
            .i(Instruction::Call(self.layout.yield_))
            .i(Instruction::I32Const(EXIT));
    }

    /// The `[async-lift]` export: start with an empty buffer and run.
    fn driver_body(&self) -> Function {
        let g = self.globals;
        let buffer = self.buffer();
        let params = self.lift_params.len() as u32;
        let result = params;
        let mut b = Body::new();
        b.i(Instruction::I32Const(buffer))
            .i(Instruction::I32Const(buffer + 8))
            .i(Instruction::I32Store(mem32(0)))
            .i(Instruction::I32Const(buffer))
            .i(Instruction::I32Const(buffer + BUFFER_BYTES))
            .i(Instruction::I32Store(mem32(4)))
            .i(Instruction::I32Const(buffer))
            .i(Instruction::GlobalSet(g.data))
            .i(Instruction::I32Const(0))
            .i(Instruction::GlobalSet(g.state));
        for param in 0..params {
            b.i(Instruction::LocalGet(param));
        }
        b.i(Instruction::Call(self.layout.entry))
            .i(Instruction::LocalSet(result));
        self.finish_run(&mut b, result);
        b.finish(vec![(1, ValType::I32)])
    }

    /// The `[callback][async-lift]` export: keep the event, rewind and run.
    fn callback_body(&self) -> Function {
        let g = self.globals;
        let result = 3;
        let mut b = Body::new();
        b.i(Instruction::LocalGet(0))
            .i(Instruction::GlobalSet(g.event))
            .i(Instruction::LocalGet(1))
            .i(Instruction::GlobalSet(g.payload))
            .i(Instruction::LocalGet(2))
            .i(Instruction::GlobalSet(g.payload2))
            .i(Instruction::I32Const(REWINDING))
            .i(Instruction::GlobalSet(g.state));
        // The entry's parameters come back from its frame.
        for ty in &self.lift_params {
            b.i(match ty {
                ValType::I64 => Instruction::I64Const(0),
                _ => Instruction::I32Const(0),
            });
        }
        b.i(Instruction::Call(self.layout.entry))
            .i(Instruction::LocalSet(result));
        self.finish_run(&mut b, result);
        b.finish(vec![(1, ValType::I32)])
    }
}
