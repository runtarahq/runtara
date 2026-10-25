// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! `runtara:workflow/runtime` lowered onto `runtara:api` for the workflow API
//! guest ABI.
//!
//! The emitter's call sites speak the core signatures and result layouts of
//! the old runtime interface. On the workflow API ABI the logic imports
//! `runtara:api` instead, and each old runtime function becomes a function
//! defined in the module (a shim) with the old core signature: it calls the
//! matching `runtara:api` function with a scratch area of its own as the
//! result pointer (an `api-error` is far larger than the old `string`, so
//! the caller's result area would not hold it), then writes the old layout
//! into the caller's result area. The few old functions that are
//! composites of several endpoints (`is-cancelled`, `check-signals`) are the
//! same composites here; `is-cancelled` remembers an acknowledged stop in a
//! module global, as the old host did.
//!
//! Every offset comes from the WIT through `SizeAlign`, and every shim checks
//! that the import has the flat signature it pushes for, so a WIT change that
//! moves a field fails the compile instead of corrupting memory. The scratch
//! area sits just past the static data, below the heap, and every new result
//! fits it ([`tests`] pins that). The logic never runs two shims at once: its
//! runtime calls are synchronous.

use std::collections::BTreeMap;

use wasm_encoder::{
    BlockType, CodeSection, Function as WasmFunction, FunctionSection, Instruction, MemArg,
    TypeSection, ValType,
};
use wit_parser::abi::WasmType;
use wit_parser::{
    Function as WitFunction, Int, ManglingAndAbi, Resolve, SizeAlign, Type, TypeDefKind,
};

use super::abi::push_core_type;
use super::core_imports::DirectCoreImportIndices;
use super::{DirectCompileError, component_error};

/// The module global (after the heap pointer) that remembers an
/// acknowledged stop.
pub(super) const STOPPED_GLOBAL: u32 = 1;
/// Bytes reserved for the shims' results, below the heap.
pub(super) const API_SCRATCH_BYTES: i32 = 128;

/// One step into a type when computing a byte offset.
#[derive(Clone, Copy)]
enum Step {
    Ok,
    Err,
    Some,
    Field(&'static str),
    Case(&'static str),
}

struct Layouts<'a> {
    resolve: &'a Resolve,
    sizes: SizeAlign,
}

impl<'a> Layouts<'a> {
    fn new(resolve: &'a Resolve) -> Self {
        let mut sizes = SizeAlign::default();
        sizes.fill(resolve);
        Self { resolve, sizes }
    }

    /// Byte offset of `path` inside a value of type `ty`.
    fn offset(&self, ty: &Type, path: &[Step]) -> Result<u64, DirectCompileError> {
        let mut ty = *ty;
        let mut offset = 0u64;
        for step in path {
            let Type::Id(mut id) = ty else {
                return Err(component_error("api shim: offset path into a scalar"));
            };
            while let TypeDefKind::Type(Type::Id(next)) = &self.resolve.types[id].kind {
                id = *next;
            }
            let (delta, next) = match (&self.resolve.types[id].kind, step) {
                (TypeDefKind::Result(result), Step::Ok | Step::Err) => {
                    let payload = self
                        .sizes
                        .payload_offset(Int::U8, [result.ok.as_ref(), result.err.as_ref()]);
                    let next = if matches!(step, Step::Ok) {
                        result.ok
                    } else {
                        result.err
                    };
                    (payload.size_wasm32(), next)
                }
                (TypeDefKind::Option(inner), Step::Some) => (
                    self.sizes
                        .payload_offset(Int::U8, [None, Some(inner)])
                        .size_wasm32(),
                    Some(*inner),
                ),
                (TypeDefKind::Record(record), Step::Field(name)) => {
                    let offsets = self
                        .sizes
                        .field_offsets(record.fields.iter().map(|f| &f.ty));
                    let index = record
                        .fields
                        .iter()
                        .position(|field| field.name == *name)
                        .ok_or_else(|| component_error(format!("api shim: no field `{name}`")))?;
                    (
                        offsets[index].0.size_wasm32(),
                        Some(record.fields[index].ty),
                    )
                }
                (TypeDefKind::Variant(variant), Step::Case(name)) => {
                    let case = variant
                        .cases
                        .iter()
                        .find(|case| case.name == *name)
                        .ok_or_else(|| component_error(format!("api shim: no case `{name}`")))?;
                    (
                        self.sizes
                            .payload_offset(
                                variant.tag(),
                                variant.cases.iter().map(|c| c.ty.as_ref()),
                            )
                            .size_wasm32(),
                        case.ty,
                    )
                }
                _ => {
                    return Err(component_error(
                        "api shim: offset path does not match the type",
                    ));
                }
            };
            offset += delta as u64;
            ty =
                next.ok_or_else(|| component_error("api shim: offset path into an empty payload"))?;
        }
        Ok(offset)
    }

    #[cfg(test)]
    fn size(&self, ty: &Type) -> u64 {
        self.sizes.size(ty).size_wasm32() as u64
    }
}

fn function<'r>(
    resolve: &'r Resolve,
    package: &str,
    interface: &str,
    name: &str,
) -> Result<&'r WitFunction, DirectCompileError> {
    resolve
        .packages
        .iter()
        .find(|(_, p)| format!("{}:{}", p.name.namespace, p.name.name) == package)
        .and_then(|(_, p)| p.interfaces.get(interface))
        .and_then(|id| resolve.interfaces[*id].functions.get(name))
        .ok_or_else(|| component_error(format!("api shim: no {package}/{interface}.{name}")))
}

fn result_type(function: &WitFunction) -> Result<Type, DirectCompileError> {
    function
        .result
        .ok_or_else(|| component_error(format!("api shim: `{}` has no result", function.name)))
}

fn mem(offset: u64, align: u32) -> MemArg {
    MemArg {
        offset,
        align,
        memory_index: 0,
    }
}

/// The body of one shim, with the retptr parameter and temporaries named.
struct Shim<'l> {
    body: WasmFunction,
    retptr: u32,
    /// Address of the scratch area the `runtara:api` call writes into.
    scratch: i32,
    /// First i32 temporary local; `SCRATCH_I32` of them follow.
    tmp: u32,
    /// The i64 temporary local.
    tmp_i64: u32,
    layouts: &'l Layouts<'l>,
}

const SCRATCH_I32: u32 = 12;
const SCRATCH_I64: u32 = 1;

impl Shim<'_> {
    fn new<'l>(params: usize, scratch: i32, layouts: &'l Layouts<'l>) -> Shim<'l> {
        let params = params as u32;
        Shim {
            body: WasmFunction::new([(SCRATCH_I32, ValType::I32), (SCRATCH_I64, ValType::I64)]),
            retptr: params - 1,
            scratch,
            tmp: params,
            tmp_i64: params + SCRATCH_I32,
            layouts,
        }
    }

    fn i(&mut self, instruction: Instruction) -> &mut Self {
        self.body.instruction(&instruction);
        self
    }

    fn local(&self, n: u32) -> u32 {
        assert!(n < SCRATCH_I32, "api shim temporary local {n}");
        self.tmp + n
    }

    /// Load from the scratch area (the `runtara:api` result).
    fn load_i32(&mut self, offset: u64, into: u32) {
        let scratch = self.scratch;
        self.i(Instruction::I32Const(scratch))
            .i(Instruction::I32Load(mem(offset, 2)))
            .i(Instruction::LocalSet(into));
    }

    fn load_u8(&mut self, offset: u64, into: u32) {
        let scratch = self.scratch;
        self.i(Instruction::I32Const(scratch))
            .i(Instruction::I32Load8U(mem(offset, 0)))
            .i(Instruction::LocalSet(into));
    }

    /// Store into the scratch area (arguments built in memory).
    fn store_scratch_i32(&mut self, offset: u64, from: u32) {
        let scratch = self.scratch;
        self.i(Instruction::I32Const(scratch))
            .i(Instruction::LocalGet(from))
            .i(Instruction::I32Store(mem(offset, 2)));
    }

    fn store_i32(&mut self, offset: u64, from: u32) {
        let retptr = self.retptr;
        self.i(Instruction::LocalGet(retptr))
            .i(Instruction::LocalGet(from))
            .i(Instruction::I32Store(mem(offset, 2)));
    }

    fn store_u8(&mut self, offset: u64, from: u32) {
        let retptr = self.retptr;
        self.i(Instruction::LocalGet(retptr))
            .i(Instruction::LocalGet(from))
            .i(Instruction::I32Store8(mem(offset, 0)));
    }

    fn store_const_u8(&mut self, offset: u64, value: i32) {
        let retptr = self.retptr;
        self.i(Instruction::LocalGet(retptr))
            .i(Instruction::I32Const(value))
            .i(Instruction::I32Store8(mem(offset, 0)));
    }

    fn store_const_i32(&mut self, offset: u64, value: i32) {
        let retptr = self.retptr;
        self.i(Instruction::LocalGet(retptr))
            .i(Instruction::I32Const(value))
            .i(Instruction::I32Store(mem(offset, 2)));
    }

    /// Call `import` with `args` (locals, then the scratch area as its
    /// result pointer when it takes one).
    fn call(&mut self, import: &ApiImport, args: &[u32], with_result: bool) {
        for arg in args {
            self.i(Instruction::LocalGet(*arg));
        }
        if with_result {
            let scratch = self.scratch;
            self.i(Instruction::I32Const(scratch));
        }
        self.i(Instruction::Call(import.index));
    }

    /// After a call into `new` (a `result<_, api-error>`): when it failed,
    /// write the old `result<_, string>` error (the envelope's message) and
    /// return.
    fn on_error_return(&mut self, new: &Type, old: &Type) -> Result<(), DirectCompileError> {
        let message = self
            .layouts
            .offset(new, &[Step::Err, Step::Field("message")])?;
        let old_err = self.layouts.offset(old, &[Step::Err])?;
        let (ptr, len) = (self.local(0), self.local(1));
        let scratch = self.scratch;
        self.i(Instruction::I32Const(scratch))
            .i(Instruction::I32Load8U(mem(0, 0)))
            .i(Instruction::If(BlockType::Empty));
        self.load_i32(message, ptr);
        self.load_i32(message + 4, len);
        self.store_const_u8(0, 1);
        self.store_i32(old_err, ptr);
        self.store_i32(old_err + 4, len);
        self.i(Instruction::Return).i(Instruction::End);
        Ok(())
    }

    /// Like [`Self::on_error_return`], but a failure traps (the old host
    /// trapped when it could not confirm the call).
    fn on_error_trap(&mut self) {
        let scratch = self.scratch;
        self.i(Instruction::I32Const(scratch))
            .i(Instruction::I32Load8U(mem(0, 0)))
            .i(Instruction::If(BlockType::Empty))
            .i(Instruction::Unreachable)
            .i(Instruction::End);
    }

    /// Push whether the string in locals `ptr`/`len` equals `literal`.
    fn push_str_eq(&mut self, ptr: u32, len: u32, literal: &str) {
        self.i(Instruction::LocalGet(len))
            .i(Instruction::I32Const(literal.len() as i32))
            .i(Instruction::I32Eq)
            .i(Instruction::If(BlockType::Result(ValType::I32)))
            .i(Instruction::I32Const(1));
        for (n, byte) in literal.bytes().enumerate() {
            self.i(Instruction::LocalGet(ptr))
                .i(Instruction::I32Load8U(mem(n as u64, 0)))
                .i(Instruction::I32Const(i32::from(byte)))
                .i(Instruction::I32Eq)
                .i(Instruction::I32And);
        }
        self.i(Instruction::Else)
            .i(Instruction::I32Const(0))
            .i(Instruction::End);
    }

    /// When `applied` and the signal in `ty_ptr`/`ty_len` is a stop, remember
    /// it in [`STOPPED_GLOBAL`].
    fn remember_stop(&mut self, applied: u32, ty_ptr: u32, ty_len: u32) {
        self.i(Instruction::LocalGet(applied));
        self.push_str_eq(ty_ptr, ty_len, "cancel");
        self.push_str_eq(ty_ptr, ty_len, "shutdown");
        self.i(Instruction::I32Or)
            .i(Instruction::I32And)
            .i(Instruction::If(BlockType::Empty))
            .i(Instruction::I32Const(1))
            .i(Instruction::GlobalSet(STOPPED_GLOBAL))
            .i(Instruction::End);
    }

    /// End the body; falling off the end is success.
    fn finish(mut self) -> WasmFunction {
        self.store_const_u8(0, 0);
        self.i(Instruction::End);
        self.body
    }
}

/// One imported `runtara:api` function.
#[derive(Debug, Clone)]
pub(super) struct ApiImport {
    pub(super) index: u32,
    pub(super) params: Vec<WasmType>,
}

/// Every old runtime function the emitter calls, by name.
const SHIMMED: [&str; 17] = [
    "custom-event",
    "debug-mode-enabled",
    "breakpoint-pause",
    "heartbeat",
    "instance-id",
    "poll-signal",
    "is-cancelled",
    "check-signals",
    "register-input",
    "poll-input",
    "close-input",
    "now-ms",
    "get-checkpoint",
    "checkpoint",
    "handle-checkpoint-signal",
    "record-retry-attempt",
    "blocking-sleep",
];

/// Define a shim for every old runtime function and point its slot at it.
/// Runs after every import is declared, before any other defined function.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_runtime_shims(
    resolve: &Resolve,
    api: &BTreeMap<String, ApiImport>,
    indices: &mut DirectCoreImportIndices,
    types: &mut TypeSection,
    type_count: &mut u32,
    functions: &mut FunctionSection,
    code: &mut CodeSection,
    imported_function_count: u32,
    next_defined_function: &mut u32,
    scratch: i32,
) -> Result<(), DirectCompileError> {
    let layouts = Layouts::new(resolve);
    let mangling = ManglingAndAbi::Standard32;
    let import = |key: &str, expected: &[WasmType]| -> Result<&ApiImport, DirectCompileError> {
        let import = api
            .get(key)
            .ok_or_else(|| component_error(format!("api shim: runtara:api {key} not imported")))?;
        if import.params != expected {
            return Err(component_error(format!(
                "api shim: runtara:api {key} has flat params {:?}, expected {expected:?}",
                import.params
            )));
        }
        Ok(import)
    };
    let api_result = |interface: &str, name: &str| -> Result<Type, DirectCompileError> {
        result_type(function(resolve, "runtara:api", interface, name)?)
    };
    use WasmType::{I32, I64, Length as L, Pointer as P};

    for name in SHIMMED {
        let old_fn = function(resolve, "runtara:workflow", "runtime", name)?;
        let old_sig = resolve.wasm_signature(mangling.import_variant(), old_fn);
        if !old_sig.retptr {
            return Err(component_error(format!(
                "api shim: runtime.{name} has no retptr"
            )));
        }
        let old = result_type(old_fn)?;
        let mut shim = Shim::new(old_sig.params.len(), scratch, &layouts);
        let param = |n: u32| n;
        match name {
            "custom-event" => {
                // One event, built where the result goes: arguments are read
                // when the call starts, before anything is written back.
                let events = api_result("events", "append")?;
                let event_ty = function(resolve, "runtara:api", "events", "append")?.params[0].ty;
                let Type::Id(list) = event_ty else {
                    return Err(component_error("api shim: events.append takes a list"));
                };
                let TypeDefKind::List(event) = resolve.types[list].kind else {
                    return Err(component_error("api shim: events.append takes a list"));
                };
                let kind = layouts.offset(&event, &[Step::Field("kind")])?;
                let payload = layouts.offset(&event, &[Step::Field("payload")])?;
                for (offset, local) in [(kind, 0), (kind + 4, 1), (payload, 2), (payload + 4, 3)] {
                    shim.store_scratch_i32(offset, param(local));
                }
                let (list, one) = (shim.local(2), shim.local(3));
                shim.i(Instruction::I32Const(scratch))
                    .i(Instruction::LocalSet(list))
                    .i(Instruction::I32Const(1))
                    .i(Instruction::LocalSet(one));
                let append = import("events.append", &[P, L, P])?;
                shim.call(append, &[list, one], true);
                shim.on_error_return(&events, &old)?;
            }
            "debug-mode-enabled" | "instance-id" => {
                let info = api_result("run", "info")?;
                shim.call(import("run.info", &[P])?, &[], true);
                shim.on_error_return(&info, &old)?;
                let ok = layouts.offset(&old, &[Step::Ok])?;
                if name == "debug-mode-enabled" {
                    let debug = layouts.offset(&info, &[Step::Ok, Step::Field("debug")])?;
                    let value = shim.local(2);
                    shim.load_u8(debug, value);
                    shim.store_u8(ok, value);
                } else {
                    let id = layouts.offset(&info, &[Step::Ok, Step::Field("instance-id")])?;
                    let (ptr, len) = (shim.local(2), shim.local(3));
                    shim.load_i32(id, ptr);
                    shim.load_i32(id + 4, len);
                    shim.store_i32(ok, ptr);
                    shim.store_i32(ok + 4, len);
                }
            }
            "breakpoint-pause" => {
                let breakpoint = api_result("debug", "breakpoint")?;
                let wait = shim.tmp_i64;
                shim.i(Instruction::I64Const(0))
                    .i(Instruction::LocalSet(wait));
                shim.call(import("debug.breakpoint", &[I64, P])?, &[wait], true);
                shim.on_error_return(&breakpoint, &old)?;
            }
            "heartbeat" => {
                let heartbeat = api_result("time", "heartbeat")?;
                shim.call(import("time.heartbeat", &[P])?, &[], true);
                shim.on_error_return(&heartbeat, &old)?;
            }
            "now-ms" => {
                let now = api_result("time", "now-ms")?;
                shim.call(import("time.now-ms", &[P])?, &[], true);
                shim.on_error_return(&now, &old)?;
                let (from, to) = (
                    layouts.offset(&now, &[Step::Ok])?,
                    layouts.offset(&old, &[Step::Ok])?,
                );
                let value = shim.tmp_i64;
                let retptr = shim.retptr;
                shim.i(Instruction::I32Const(scratch))
                    .i(Instruction::I64Load(mem(from, 3)))
                    .i(Instruction::LocalSet(value))
                    .i(Instruction::LocalGet(retptr))
                    .i(Instruction::LocalGet(value))
                    .i(Instruction::I64Store(mem(to, 3)));
            }
            "poll-signal" => {
                let poll = api_result("signals", "poll-lifecycle")?;
                shim.call(import("signals.poll-lifecycle", &[P])?, &[], true);
                shim.on_error_return(&poll, &old)?;
                write_polled_signal(&mut shim, &poll, &old)?;
            }
            "is-cancelled" | "check-signals" => {
                let poll = api_result("signals", "poll-lifecycle")?;
                let ack = api_result("signals", "ack-lifecycle")?;
                let ok = layouts.offset(&old, &[Step::Ok])?;
                let applied = shim.local(6);
                if name == "is-cancelled" {
                    shim.i(Instruction::GlobalGet(STOPPED_GLOBAL))
                        .i(Instruction::If(BlockType::Empty));
                    shim.store_const_u8(0, 0);
                    shim.store_const_u8(ok, 1);
                    shim.i(Instruction::Return).i(Instruction::End);
                }
                shim.call(import("signals.poll-lifecycle", &[P])?, &[], true);
                shim.on_error_return(&poll, &old)?;
                let some = layouts.offset(&poll, &[Step::Ok])?;
                let notice = [Step::Ok, Step::Some, Step::Field("notice")];
                let signal_type = layouts.offset(
                    &poll,
                    &[notice[0], notice[1], notice[2], Step::Field("signal-type")],
                )?;
                let command = layouts.offset(
                    &poll,
                    &[notice[0], notice[1], notice[2], Step::Field("command-id")],
                )?;
                let (tag, ty_ptr, ty_len, cmd_ptr, cmd_len) = (
                    shim.local(2),
                    shim.local(3),
                    shim.local(4),
                    shim.local(5),
                    shim.local(7),
                );
                shim.load_u8(some, tag);
                shim.load_i32(signal_type, ty_ptr);
                shim.load_i32(signal_type + 4, ty_len);
                shim.load_i32(command, cmd_ptr);
                shim.load_i32(command + 4, cmd_len);
                // Nothing pending (or, for `is-cancelled`, not a cancel):
                // not stopped.
                shim.i(Instruction::LocalGet(tag)).i(Instruction::I32Eqz);
                if name == "is-cancelled" {
                    shim.push_str_eq(ty_ptr, ty_len, "cancel");
                    shim.i(Instruction::I32Eqz).i(Instruction::I32Or);
                }
                shim.i(Instruction::If(BlockType::Empty));
                shim.store_const_u8(0, 0);
                shim.store_const_u8(ok, 0);
                shim.i(Instruction::Return).i(Instruction::End);
                shim.call(
                    import("signals.ack-lifecycle", &[P, L, P, L, P])?,
                    &[cmd_ptr, cmd_len, ty_ptr, ty_len],
                    true,
                );
                shim.on_error_return(&ack, &old)?;
                shim.load_u8(layouts.offset(&ack, &[Step::Ok])?, applied);
                shim.remember_stop(applied, ty_ptr, ty_len);
                shim.store_u8(ok, applied);
            }
            "handle-checkpoint-signal" => {
                let ack = api_result("signals", "ack-lifecycle")?;
                let applied = shim.local(2);
                // Old: (signal-type, command-id); new: (command-id, signal-type).
                shim.call(
                    import("signals.ack-lifecycle", &[P, L, P, L, P])?,
                    &[param(2), param(3), param(0), param(1)],
                    true,
                );
                shim.on_error_return(&ack, &old)?;
                shim.load_u8(layouts.offset(&ack, &[Step::Ok])?, applied);
                shim.remember_stop(applied, param(0), param(1));
                shim.store_u8(layouts.offset(&old, &[Step::Ok])?, applied);
            }
            "register-input" => {
                let register = api_result("inputs", "register")?;
                shim.call(
                    import("inputs.register", &[P, L, I32, I64, P])?,
                    &[param(0), param(1), param(2), param(3)],
                    true,
                );
                shim.on_error_return(&register, &old)?;
            }
            "poll-input" | "close-input" => {
                let (key, ty) = if name == "poll-input" {
                    ("inputs.poll", api_result("inputs", "poll")?)
                } else {
                    ("inputs.close", api_result("inputs", "close")?)
                };
                if name == "poll-input" {
                    let wait = shim.tmp_i64;
                    shim.i(Instruction::I64Const(0))
                        .i(Instruction::LocalSet(wait));
                    shim.call(
                        import(key, &[P, L, I64, P])?,
                        &[param(0), param(1), wait],
                        true,
                    );
                    shim.on_error_return(&ty, &old)?;
                } else {
                    shim.call(import(key, &[P, L, P])?, &[param(0), param(1)], true);
                    shim.on_error_trap();
                }
                // `input-read` and the old `input-state` are the same variant.
                let (from, to) = (
                    layouts.offset(&ty, &[Step::Ok])?,
                    layouts.offset(&old, &[Step::Ok])?,
                );
                let (from_payload, to_payload) = (
                    layouts.offset(&ty, &[Step::Ok, Step::Case("accepted")])?,
                    layouts.offset(&old, &[Step::Ok, Step::Case("accepted")])?,
                );
                let (tag, ptr, len) = (shim.local(2), shim.local(3), shim.local(4));
                shim.load_u8(from, tag);
                shim.load_i32(from_payload, ptr);
                shim.load_i32(from_payload + 4, len);
                shim.store_u8(to, tag);
                shim.store_i32(to_payload, ptr);
                shim.store_i32(to_payload + 4, len);
            }
            "get-checkpoint" => {
                let get = api_result("checkpoints", "get")?;
                shim.call(
                    import("checkpoints.get", &[P, L, P])?,
                    &[param(0), param(1)],
                    true,
                );
                shim.on_error_return(&get, &old)?;
                copy_option_list(&mut shim, &get, &old)?;
            }
            "checkpoint" => {
                let put = api_result("checkpoints", "put")?;
                shim.call(
                    import("checkpoints.put", &[P, L, P, L, P])?,
                    &[param(0), param(1), param(2), param(3)],
                    true,
                );
                shim.on_error_return(&put, &old)?;
                write_checkpoint_result(&mut shim, &put, &old)?;
            }
            "record-retry-attempt" => {
                let record = api_result("checkpoints", "record-retry-attempt")?;
                shim.call(
                    import(
                        "checkpoints.record-retry-attempt",
                        &[P, L, I32, I32, P, L, P],
                    )?,
                    &[param(0), param(1), param(2), param(3), param(4), param(5)],
                    true,
                );
                shim.on_error_return(&record, &old)?;
            }
            "blocking-sleep" => {
                shim.call(import("time.sleep", &[I64])?, &[param(0)], false);
            }
            other => unreachable!("unshimmed runtime function {other}"),
        }

        let index = imported_function_count + *next_defined_function;
        *next_defined_function += 1;
        functions.function(push_core_type(
            types,
            type_count,
            &old_sig.params,
            &old_sig.results,
        ));
        code.function(&shim.finish());
        indices.set_runtime_shim(name, index);
    }
    Ok(())
}

/// `option<list<u8>>` from `new`'s ok payload into `old`'s.
fn copy_option_list(shim: &mut Shim, new: &Type, old: &Type) -> Result<(), DirectCompileError> {
    let layouts = shim.layouts;
    let (from, to) = (
        layouts.offset(new, &[Step::Ok])?,
        layouts.offset(old, &[Step::Ok])?,
    );
    let (from_list, to_list) = (
        layouts.offset(new, &[Step::Ok, Step::Some])?,
        layouts.offset(old, &[Step::Ok, Step::Some])?,
    );
    let (tag, ptr, len) = (shim.local(2), shim.local(3), shim.local(4));
    shim.load_u8(from, tag);
    shim.load_i32(from_list, ptr);
    shim.load_i32(from_list + 4, len);
    shim.store_u8(to, tag);
    shim.store_i32(to_list, ptr);
    shim.store_i32(to_list + 4, len);
    Ok(())
}

/// `option<lifecycle-signal>` (new) as `option<signal-info>` (old).
fn write_polled_signal(shim: &mut Shim, new: &Type, old: &Type) -> Result<(), DirectCompileError> {
    use Step::{Field, Ok, Some};
    let layouts = shim.layouts;
    // (new path, old path) of each string or list field.
    let fields: [(&[Step], &[Step]); 3] = [
        (
            &[Ok, Some, Field("notice"), Field("signal-type")],
            &[Ok, Some, Field("signal-type")],
        ),
        (
            &[Ok, Some, Field("notice"), Field("command-id")],
            &[Ok, Some, Field("command-id")],
        ),
        (&[Ok, Some, Field("payload")], &[Ok, Some, Field("payload")]),
    ];
    let tag = shim.local(2);
    shim.load_u8(layouts.offset(new, &[Ok])?, tag);
    // Locals 3..=8: each field's ptr and len; 9..=11: the checkpoint id's
    // option tag, ptr and len.
    let mut loaded = Vec::new();
    for (n, (from, _)) in fields.iter().enumerate() {
        let offset = layouts.offset(new, from)?;
        let (ptr, len) = (shim.local(3 + 2 * n as u32), shim.local(4 + 2 * n as u32));
        shim.load_i32(offset, ptr);
        shim.load_i32(offset + 4, len);
        loaded.push((ptr, len));
    }
    let new_checkpoint: &[Step] = &[Ok, Some, Field("notice"), Field("checkpoint-id")];
    let checkpoint = layouts.offset(new, new_checkpoint)?;
    let checkpoint_value = layouts.offset(
        new,
        &[Ok, Some, Field("notice"), Field("checkpoint-id"), Some],
    )?;
    let (cp_tag, cp_ptr, cp_len) = (shim.local(9), shim.local(10), shim.local(11));
    shim.load_u8(checkpoint, cp_tag);
    shim.load_i32(checkpoint_value, cp_ptr);
    shim.load_i32(checkpoint_value + 4, cp_len);

    shim.store_u8(layouts.offset(old, &[Ok])?, tag);
    for (n, (_, to)) in fields.iter().enumerate() {
        let offset = layouts.offset(old, to)?;
        let (ptr, len) = loaded[n];
        shim.store_i32(offset, ptr);
        shim.store_i32(offset + 4, len);
    }
    shim.store_u8(
        layouts.offset(old, &[Ok, Some, Field("checkpoint-id")])?,
        cp_tag,
    );
    let old_value = layouts.offset(old, &[Ok, Some, Field("checkpoint-id"), Some])?;
    shim.store_i32(old_value, cp_ptr);
    shim.store_i32(old_value + 4, cp_len);
    Result::Ok(())
}

/// `checkpoint-written` (new) as the old `checkpoint-result`. Call sites read
/// `found`, `state` and the pending signal's type and command id; the old
/// result's signal payload and custom signal are left empty.
fn write_checkpoint_result(
    shim: &mut Shim,
    new: &Type,
    old: &Type,
) -> Result<(), DirectCompileError> {
    let layouts = shim.layouts;
    let field = |name| [Step::Ok, Step::Field(name)];
    let stored = layouts.offset(new, &field("stored"))?;
    let state = layouts.offset(new, &field("state"))?;
    let pending = layouts.offset(new, &field("pending-signal"))?;
    let pending_notice = [Step::Ok, Step::Field("pending-signal"), Step::Some];
    let signal_type = layouts.offset(
        new,
        &[
            pending_notice[0],
            pending_notice[1],
            pending_notice[2],
            Step::Field("signal-type"),
        ],
    )?;
    let command = layouts.offset(
        new,
        &[
            pending_notice[0],
            pending_notice[1],
            pending_notice[2],
            Step::Field("command-id"),
        ],
    )?;

    let (found, state_ptr, state_len, tag) =
        (shim.local(2), shim.local(3), shim.local(4), shim.local(5));
    let (ty_ptr, ty_len, cmd_ptr, cmd_len) =
        (shim.local(6), shim.local(7), shim.local(8), shim.local(9));
    shim.load_u8(stored, found);
    shim.i(Instruction::LocalGet(found))
        .i(Instruction::I32Eqz)
        .i(Instruction::LocalSet(found));
    shim.load_i32(state, state_ptr);
    shim.load_i32(state + 4, state_len);
    shim.load_u8(pending, tag);
    shim.load_i32(signal_type, ty_ptr);
    shim.load_i32(signal_type + 4, ty_len);
    shim.load_i32(command, cmd_ptr);
    shim.load_i32(command + 4, cmd_len);

    shim.store_u8(layouts.offset(old, &field("found"))?, found);
    let old_state = layouts.offset(old, &field("state"))?;
    shim.store_i32(old_state, state_ptr);
    shim.store_i32(old_state + 4, state_len);
    let old_pending = [Step::Ok, Step::Field("pending-signal")];
    shim.store_u8(layouts.offset(old, &old_pending)?, tag);
    let signal = |name| {
        [
            Step::Ok,
            Step::Field("pending-signal"),
            Step::Some,
            Step::Field(name),
        ]
    };
    for (name, ptr, len) in [
        ("signal-type", ty_ptr, ty_len),
        ("command-id", cmd_ptr, cmd_len),
    ] {
        let offset = layouts.offset(old, &signal(name))?;
        shim.store_i32(offset, ptr);
        shim.store_i32(offset + 4, len);
    }
    let payload = layouts.offset(old, &signal("payload"))?;
    shim.store_const_i32(payload, 0);
    shim.store_const_i32(payload + 4, 0);
    shim.store_const_u8(layouts.offset(old, &signal("checkpoint-id"))?, 0);
    shim.store_const_u8(layouts.offset(old, &field("custom-signal"))?, 0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve() -> Resolve {
        let mut resolve = runtara_wit::resolve().unwrap();
        resolve
            .push_str("runtara-api.wit", runtara_wit::api::WIT)
            .unwrap();
        resolve
    }

    /// Every `runtara:api` result a shim reads fits the scratch area.
    #[test]
    fn every_api_result_fits_the_scratch_area() {
        let resolve = resolve();
        let layouts = Layouts::new(&resolve);
        for (old, new) in [
            ("custom-event", ("events", "append")),
            ("debug-mode-enabled", ("run", "info")),
            ("instance-id", ("run", "info")),
            ("breakpoint-pause", ("debug", "breakpoint")),
            ("heartbeat", ("time", "heartbeat")),
            ("now-ms", ("time", "now-ms")),
            ("poll-signal", ("signals", "poll-lifecycle")),
            ("is-cancelled", ("signals", "ack-lifecycle")),
            ("check-signals", ("signals", "poll-lifecycle")),
            ("handle-checkpoint-signal", ("signals", "ack-lifecycle")),
            ("register-input", ("inputs", "register")),
            ("poll-input", ("inputs", "poll")),
            ("close-input", ("inputs", "close")),
            ("get-checkpoint", ("checkpoints", "get")),
            ("checkpoint", ("checkpoints", "put")),
            (
                "record-retry-attempt",
                ("checkpoints", "record-retry-attempt"),
            ),
        ] {
            let old_ty =
                result_type(function(&resolve, "runtara:workflow", "runtime", old).unwrap())
                    .unwrap();
            let new_ty =
                result_type(function(&resolve, "runtara:api", new.0, new.1).unwrap()).unwrap();
            let _ = old_ty;
            assert!(
                layouts.size(&new_ty) <= API_SCRATCH_BYTES as u64,
                "{old}: runtara:api {}.{} result is {} bytes",
                new.0,
                new.1,
                layouts.size(&new_ty),
            );
        }
    }

    #[test]
    fn offsets_follow_the_wit() {
        let resolve = resolve();
        let layouts = Layouts::new(&resolve);
        let old =
            result_type(function(&resolve, "runtara:workflow", "runtime", "checkpoint").unwrap())
                .unwrap();
        // The emitter's own constants for the old checkpoint result.
        assert_eq!(
            layouts
                .offset(&old, &[Step::Ok, Step::Field("found")])
                .unwrap(),
            super::super::DIRECT_CHECKPOINT_FOUND_OFFSET
        );
        assert_eq!(
            layouts
                .offset(&old, &[Step::Ok, Step::Field("pending-signal")])
                .unwrap(),
            super::super::DIRECT_CHECKPOINT_PENDING_SIGNAL_TAG_OFFSET
        );
        assert_eq!(
            layouts
                .offset(
                    &old,
                    &[
                        Step::Ok,
                        Step::Field("pending-signal"),
                        Step::Some,
                        Step::Field("signal-type")
                    ]
                )
                .unwrap(),
            super::super::DIRECT_CHECKPOINT_SIGNAL_TYPE_PTR_OFFSET
        );
        let now =
            result_type(function(&resolve, "runtara:api", "time", "now-ms").unwrap()).unwrap();
        assert_eq!(layouts.offset(&now, &[Step::Ok]).unwrap(), 8);
        assert_eq!(
            layouts
                .offset(&now, &[Step::Err, Step::Field("message")])
                .unwrap(),
            16
        );
    }
}
