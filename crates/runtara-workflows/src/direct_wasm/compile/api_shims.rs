// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! The emitter's run-service helpers over `runtara:api`.
//!
//! The emitter's call sites speak the emitter's own convention: a synchronous
//! call whose `result<T, string>` lands at the caller's result pointer
//! ([`native`] describes each helper's core signature and layout). Each
//! helper is a function defined in the module: it calls the matching
//! `runtara:api` function with a scratch area of its own as the result
//! pointer (an `api-error` is far larger than a `string`, so the caller's
//! result area would not hold it), then writes the caller's layout.
//! `is-cancelled` and `check-signals` call the host's own composites
//! (`/v1/signals/lifecycle/cancelled` and `/check`), which keep the host's
//! cancel flag.
//!
//! Every `runtara:api` offset comes from its WIT through `SizeAlign`, and
//! every helper checks that the import has the flat signature it pushes for,
//! so a WIT change that moves a field fails the compile instead of corrupting
//! memory. The scratch
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

/// Bytes reserved for the shims' results, below the heap.
pub(super) const API_SCRATCH_BYTES: i32 = 128;

/// The entered operation, right after the scratch area. Operation-scoped
/// sites run one at a time in a store, so one cell holds the operation the
/// old implicit `runtara:workflow/operation` named:
/// `state` (0 none, 1 entered, 2 left but not yet released), the
/// continuation `enter` loaded (`option<list<u8>>` as tag, pointer, length)
/// and the 64-byte hex operation id.
pub(super) const OPERATION_CELL_BYTES: i32 = 80;
pub(super) const OPERATION_STATE: u64 = 0;
pub(super) const OPERATION_CONTINUATION_TAG: u64 = 4;
pub(super) const OPERATION_CONTINUATION_PTR: u64 = 8;
pub(super) const OPERATION_CONTINUATION_LEN: u64 = 12;
pub(super) const OPERATION_ID: u64 = 16;
/// A `runtara:api` operation id is a sha256 hex digest.
pub(super) const OPERATION_ID_LEN: i32 = 64;
const ENTERED: i32 = 1;
const LEFT: i32 = 2;

/// One step into a type when computing a byte offset.
#[derive(Clone, Copy)]
pub(super) enum Step {
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
    /// write the caller's `result<_, string>` error (the envelope's message)
    /// at `old_err` of its result area, and return.
    fn on_error_return(&mut self, new: &Type, old_err: u64) -> Result<(), DirectCompileError> {
        let message = self
            .layouts
            .offset(new, &[Step::Err, Step::Field("message")])?;
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

    /// After a call into `new` (a `result<_, api-error>`): when it failed,
    /// write the old `result<_, string>` error as the `{code, message,
    /// retryAfterMs}` JSON the wait and state errors read (built by the
    /// stdlib's `api-error-json`, at `stdlib`), and return.
    /// The error string goes to `old_err` of the caller's result area.
    fn on_error_return_json(
        &mut self,
        new: &Type,
        old_err: u64,
        stdlib: u32,
    ) -> Result<(), DirectCompileError> {
        let layouts = self.layouts;
        let code = layouts.offset(new, &[Step::Err, Step::Field("code")])?;
        let message = layouts.offset(new, &[Step::Err, Step::Field("message")])?;
        let retry = layouts.offset(new, &[Step::Err, Step::Field("retry-after-ms")])?;
        let retry_value =
            layouts.offset(new, &[Step::Err, Step::Field("retry-after-ms"), Step::Some])?;
        let [
            code_ptr,
            code_len,
            msg_ptr,
            msg_len,
            retry_tag,
            json_ptr,
            json_len,
        ] = [0, 1, 2, 3, 4, 5, 6].map(|n| self.local(n));
        let retry_ms = self.tmp_i64;
        let scratch = self.scratch;
        self.i(Instruction::I32Const(scratch))
            .i(Instruction::I32Load8U(mem(0, 0)))
            .i(Instruction::If(BlockType::Empty));
        self.load_i32(code, code_ptr);
        self.load_i32(code + 4, code_len);
        self.load_i32(message, msg_ptr);
        self.load_i32(message + 4, msg_len);
        self.load_u8(retry, retry_tag);
        self.i(Instruction::I32Const(scratch))
            .i(Instruction::I64Load(mem(retry_value, 3)))
            .i(Instruction::LocalSet(retry_ms));
        for local in [code_ptr, code_len, msg_ptr, msg_len, retry_tag, retry_ms] {
            self.i(Instruction::LocalGet(local));
        }
        self.i(Instruction::I32Const(scratch))
            .i(Instruction::Call(stdlib));
        self.load_i32(0, json_ptr);
        self.load_i32(4, json_len);
        self.store_const_u8(0, 1);
        self.store_i32(old_err, json_ptr);
        self.store_i32(old_err + 4, json_len);
        self.i(Instruction::Return).i(Instruction::End);
        Ok(())
    }

    /// End the body; falling off the end is success.
    fn finish(mut self) -> WasmFunction {
        self.store_const_u8(0, 0);
        self.i(Instruction::End);
        self.body
    }

    /// End the body of a shim whose old function returns nothing.
    fn finish_void(mut self) -> WasmFunction {
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

/// The emitter's own core signatures and result layouts of its runtime
/// helpers: the canonical ABI of a synchronous `result<T, string>` call, the
/// result pointer last. The tag is at 0; the error string and every 4-aligned
/// `T` at 4, a `u64` at 8. The call sites read these offsets
/// (`DIRECT_RESULT_*`, `DIRECT_CHECKPOINT_*`), and a helper writes them from
/// the `runtara:api` answer.
pub(super) mod native {
    use wit_parser::abi::WasmType::{self, I32, I64, Length as L, Pointer as P};

    /// The error string, and every 4-aligned ok payload.
    pub(crate) const PAYLOAD: u64 = 4;
    /// A `u64` ok payload (`now-ms`).
    pub(crate) const U64_PAYLOAD: u64 = 8;
    /// The list of an `option<list<u8>>` or of an input's `accepted(list)`,
    /// whose tag is at [`PAYLOAD`].
    pub(crate) const OPTION_LIST: u64 = 8;

    /// A lifecycle signal: `signal-type`, `command-id`, `payload`, then an
    /// optional `checkpoint-id` (tag, then string), from its start.
    pub(crate) const SIGNAL_TYPE: u64 = 0;
    pub(crate) const SIGNAL_COMMAND: u64 = 8;
    pub(crate) const SIGNAL_PAYLOAD: u64 = 16;
    pub(crate) const SIGNAL_CHECKPOINT: u64 = 24;
    pub(crate) const SIGNAL_CHECKPOINT_VALUE: u64 = 28;
    /// `poll-signal`: an optional signal, its tag at [`PAYLOAD`].
    pub(crate) const POLLED_SIGNAL: u64 = 8;

    /// `checkpoint`: whether a checkpoint was already there, the stored
    /// state, an optional pending signal and an optional custom signal.
    pub(crate) const CHECKPOINT_FOUND: u64 = 4;
    pub(crate) const CHECKPOINT_STATE: u64 = 8;
    pub(crate) const CHECKPOINT_PENDING: u64 = 16;
    pub(crate) const CHECKPOINT_SIGNAL: u64 = 20;
    pub(crate) const CHECKPOINT_CUSTOM: u64 = 56;

    /// Where the error string of helper `name`'s result goes.
    pub(crate) fn error_at(name: &str) -> u64 {
        if name == "now-ms" {
            U64_PAYLOAD
        } else {
            PAYLOAD
        }
    }

    /// The core parameters of helper `name`, its result pointer last.
    pub(crate) fn params(name: &str) -> Vec<WasmType> {
        match name {
            "debug-mode-enabled" | "breakpoint-pause" | "heartbeat" | "instance-id"
            | "poll-signal" | "is-cancelled" | "check-signals" | "now-ms" => vec![P],
            "poll-input" | "close-input" | "get-checkpoint" => vec![P, L, P],
            "custom-event" | "checkpoint" | "handle-checkpoint-signal" => vec![P, L, P, L, P],
            "register-input" => vec![P, L, I32, I64, P],
            "record-retry-attempt" => vec![P, L, I32, I32, P, L, P],
            "blocking-sleep" => vec![I64, P],
            other => unreachable!("no runtime helper {other}"),
        }
    }
}

/// Every old runtime function the emitter calls, by name, in the order their
/// shims are defined.
pub(super) const SHIMMED: [&str; 17] = [
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
        let params = native::params(name);
        let err = native::error_at(name);
        let mut shim = Shim::new(params.len(), scratch, &layouts);
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
                shim.on_error_return(&events, err)?;
            }
            "debug-mode-enabled" | "instance-id" => {
                let info = api_result("run", "info")?;
                shim.call(import("run.info", &[P])?, &[], true);
                shim.on_error_return(&info, err)?;
                let ok = native::PAYLOAD;
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
                shim.on_error_return(&breakpoint, err)?;
            }
            "heartbeat" => {
                let heartbeat = api_result("time", "heartbeat")?;
                shim.call(import("time.heartbeat", &[P])?, &[], true);
                shim.on_error_return(&heartbeat, err)?;
            }
            "now-ms" => {
                let now = api_result("time", "now-ms")?;
                shim.call(import("time.now-ms", &[P])?, &[], true);
                shim.on_error_return(&now, err)?;
                let (from, to) = (layouts.offset(&now, &[Step::Ok])?, native::U64_PAYLOAD);
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
                shim.on_error_return(&poll, err)?;
                write_polled_signal(&mut shim, &poll)?;
            }
            "is-cancelled" | "check-signals" => {
                // The host's own composites: it keeps the cancel flag and
                // escalates an ignored cancel.
                let (key, function) = if name == "is-cancelled" {
                    ("signals.cancelled", "cancelled")
                } else {
                    ("signals.check-lifecycle", "check-lifecycle")
                };
                let checked = api_result("signals", function)?;
                shim.call(import(key, &[P])?, &[], true);
                shim.on_error_return(&checked, err)?;
                let value = shim.local(2);
                shim.load_u8(layouts.offset(&checked, &[Step::Ok])?, value);
                shim.store_u8(native::PAYLOAD, value);
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
                shim.on_error_return(&ack, err)?;
                shim.load_u8(layouts.offset(&ack, &[Step::Ok])?, applied);
                shim.store_u8(native::PAYLOAD, applied);
            }
            "register-input" => {
                let register = api_result("inputs", "register")?;
                shim.call(
                    import("inputs.register", &[P, L, I32, I64, P])?,
                    &[param(0), param(1), param(2), param(3)],
                    true,
                );
                shim.on_error_return(&register, err)?;
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
                    shim.on_error_return(&ty, err)?;
                } else {
                    shim.call(import(key, &[P, L, P])?, &[param(0), param(1)], true);
                    shim.on_error_trap();
                }
                // `input-read` as the emitter's `open | accepted(list) | closed`.
                let (from, to) = (layouts.offset(&ty, &[Step::Ok])?, native::PAYLOAD);
                let (from_payload, to_payload) = (
                    layouts.offset(&ty, &[Step::Ok, Step::Case("accepted")])?,
                    native::OPTION_LIST,
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
                shim.on_error_return(&get, err)?;
                copy_option_list(&mut shim, &get)?;
            }
            "checkpoint" => {
                let put = api_result("checkpoints", "put")?;
                shim.call(
                    import("checkpoints.put", &[P, L, P, L, P])?,
                    &[param(0), param(1), param(2), param(3)],
                    true,
                );
                shim.on_error_return(&put, err)?;
                write_checkpoint_result(&mut shim, &put)?;
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
                shim.on_error_return(&record, err)?;
            }
            "blocking-sleep" => {
                shim.call(import("time.sleep", &[I64])?, &[param(0)], false);
            }
            other => unreachable!("unshimmed runtime function {other}"),
        }

        let index = imported_function_count + *next_defined_function;
        *next_defined_function += 1;
        functions.function(push_core_type(types, type_count, &params, &[]));
        code.function(&shim.finish());
        indices.set_runtime_shim(name, index);
    }

    // Waits, state and operations, when the logic uses them.
    let waits = api.contains_key("waits.register");
    let state = api.contains_key("state.set");
    let operations = api.contains_key("operations.enter");
    let child_runs = api.contains_key("tasks.start");
    if !waits && !state && !operations && !child_runs {
        return Ok(());
    }
    // Operations, waits and state call `runtara:api` straight from helpers
    // in the emitter's own `result<T, string>` convention: the tag, then the
    // error string or the `list<u8>` at 4. A helper without a result takes no
    // result pointer.
    const NATIVE_PAYLOAD: u64 = 4;
    let mut native = |params: &[WasmType],
                      returns: bool,
                      body: &mut dyn FnMut(&mut Shim) -> Result<(), DirectCompileError>|
     -> Result<u32, DirectCompileError> {
        let mut shim = Shim::new(params.len(), scratch, &layouts);
        if !returns {
            shim.retptr = u32::MAX;
        }
        body(&mut shim)?;
        let index = imported_function_count + *next_defined_function;
        *next_defined_function += 1;
        functions.function(push_core_type(types, type_count, params, &[]));
        code.function(&if returns {
            shim.finish()
        } else {
            shim.finish_void()
        });
        Ok(index)
    };
    let native_list = |shim: &mut Shim, new: &Type| -> Result<(), DirectCompileError> {
        let from = shim.layouts.offset(new, &[Step::Ok])?;
        let (ptr, len) = (shim.local(7), shim.local(8));
        shim.load_i32(from, ptr);
        shim.load_i32(from + 4, len);
        shim.store_i32(NATIVE_PAYLOAD, ptr);
        shim.store_i32(NATIVE_PAYLOAD + 4, len);
        Ok(())
    };
    if operations {
        let cell = scratch + API_SCRATCH_BYTES;
        let id = cell + OPERATION_ID as i32;
        let enter_ty = api_result("operations", "enter")?;
        let suspend_ty = api_result("operations", "suspend")?;
        let enter = import("operations.enter", &[P, L, I32, I32, P])?;
        let suspend = import("operations.suspend", &[P, L, P, L, P, L, P])?;
        let exit = import("operations.exit", &[P, L, I32, P])?;
        let release = import("operations.release", &[P, L, P])?;
        let id_at = layouts.offset(&enter_ty, &[Step::Ok, Step::Field("operation-id")])?;
        let continuation = layouts.offset(&enter_ty, &[Step::Ok, Step::Field("continuation")])?;
        let continuation_list = layouts.offset(
            &enter_ty,
            &[Step::Ok, Step::Field("continuation"), Step::Some],
        )?;
        let code = layouts.offset(&suspend_ty, &[Step::Err, Step::Field("code")])?;
        // A refused enter, or one whose operation cannot be entered, is the
        // step error the emitter lays out: no operation is entered.
        // `enter(checkpoint-key, attempt, load, retptr)`.
        let enter_index = native(&[I32; 5], true, &mut |shim| {
            set_cell(shim, cell, OPERATION_STATE, 0);
            shim.call(enter, &[0, 1, 2, 3], true);
            shim.on_error_return(&enter_ty, NATIVE_PAYLOAD)?;
            let (ptr, len, tag) = (shim.local(2), shim.local(3), shim.local(4));
            shim.load_i32(id_at, ptr);
            shim.load_i32(id_at + 4, len);
            shim.i(Instruction::LocalGet(len))
                .i(Instruction::I32Const(OPERATION_ID_LEN))
                .i(Instruction::I32Ne)
                .i(Instruction::If(BlockType::Empty))
                .i(Instruction::Unreachable)
                .i(Instruction::End)
                .i(Instruction::I32Const(id))
                .i(Instruction::LocalGet(ptr))
                .i(Instruction::I32Const(OPERATION_ID_LEN))
                .i(Instruction::MemoryCopy {
                    src_mem: 0,
                    dst_mem: 0,
                });
            shim.load_u8(continuation, tag);
            shim.load_i32(continuation_list, ptr);
            shim.load_i32(continuation_list + 4, len);
            for (offset, local) in [
                (OPERATION_CONTINUATION_TAG, tag),
                (OPERATION_CONTINUATION_PTR, ptr),
                (OPERATION_CONTINUATION_LEN, len),
            ] {
                shim.i(Instruction::I32Const(cell))
                    .i(Instruction::LocalGet(local))
                    .i(Instruction::I32Store(mem(offset, 2)));
            }
            set_cell(shim, cell, OPERATION_STATE, ENTERED);
            Ok(())
        })?;
        // A suspension the host refuses (its caps, or no entered operation)
        // is the step error; failing to keep the continuation fails the run,
        // since a lost one would re-run the operation from scratch.
        // `suspend(state, wakes, retptr)`.
        let suspend_index = native(&[I32; 5], true, &mut |shim| {
            let old_err = NATIVE_PAYLOAD;
            let refuse = |shim: &mut Shim| {
                shim.store_const_u8(0, 1);
                shim.store_const_i32(old_err, 0);
                shim.store_const_i32(old_err + 4, 0);
                shim.i(Instruction::Return);
            };
            cell_state(shim, cell);
            shim.i(Instruction::I32Const(ENTERED))
                .i(Instruction::I32Ne)
                .i(Instruction::If(BlockType::Empty));
            refuse(shim);
            shim.i(Instruction::End);
            shim.i(Instruction::I32Const(id))
                .i(Instruction::I32Const(OPERATION_ID_LEN));
            for local in [0, 1, 2, 3] {
                shim.i(Instruction::LocalGet(local));
            }
            shim.i(Instruction::I32Const(shim.scratch))
                .i(Instruction::Call(suspend.index));
            let (ptr, len) = (shim.local(0), shim.local(1));
            let scratch = shim.scratch;
            shim.i(Instruction::I32Const(scratch))
                .i(Instruction::I32Load8U(mem(0, 0)))
                .i(Instruction::If(BlockType::Empty));
            shim.load_i32(code, ptr);
            shim.load_i32(code + 4, len);
            code_is(shim, ptr, len, "agent-invalid-suspension");
            code_is(shim, ptr, len, "not-found");
            shim.i(Instruction::I32Or)
                .i(Instruction::I32Eqz)
                .i(Instruction::If(BlockType::Empty))
                .i(Instruction::Unreachable)
                .i(Instruction::End);
            refuse(shim);
            shim.i(Instruction::End);
            Ok(())
        })?;
        // The old `exit` trapped when it could not confirm closing the
        // operation's wait and discarding its continuation. It keeps the id
        // for `release`; a refused enter left nothing to leave.
        // `exit(failed)`.
        let exit_index = native(&[I32; 1], false, &mut |shim| {
            cell_state(shim, cell);
            shim.i(Instruction::I32Const(ENTERED))
                .i(Instruction::I32Eq)
                .i(Instruction::If(BlockType::Empty))
                .i(Instruction::I32Const(id))
                .i(Instruction::I32Const(OPERATION_ID_LEN))
                .i(Instruction::LocalGet(0));
            let scratch = shim.scratch;
            shim.i(Instruction::I32Const(scratch))
                .i(Instruction::Call(exit.index));
            shim.on_error_trap();
            set_cell(shim, cell, OPERATION_STATE, LEFT);
            shim.i(Instruction::End);
            Ok(())
        })?;
        // `release` follows its own site's enter (after the result
        // checkpoint), so the cell names the operation its key does. Like
        // the old one it cannot fail: a continuation left behind goes with
        // the instance.
        // `release(checkpoint-key)`.
        let release_index = native(&[I32; 2], false, &mut |shim| {
            cell_state(shim, cell);
            shim.i(Instruction::If(BlockType::Empty))
                .i(Instruction::I32Const(id))
                .i(Instruction::I32Const(OPERATION_ID_LEN));
            let scratch = shim.scratch;
            shim.i(Instruction::I32Const(scratch))
                .i(Instruction::Call(release.index));
            set_cell(shim, cell, OPERATION_STATE, 0);
            shim.i(Instruction::End);
            Ok(())
        })?;
        indices.operation_scope = Some(super::core_imports::DirectOperationScopeImports {
            enter: enter_index,
            suspend: suspend_index,
            exit: exit_index,
            release: release_index,
        });
        indices.operation_cell = Some(cell);
    }
    if !waits && !state {
        return Ok(());
    }
    let stdlib = indices
        .stdlib_api_error_json
        .ok_or_else(|| component_error("api shim: stdlib.api-error-json not imported"))?;
    let stdlib_fn = function(resolve, "runtara:workflow-stdlib", "json", "api-error-json")?;
    let stdlib_sig = resolve.wasm_signature(mangling.import_variant(), stdlib_fn);
    if stdlib_sig.params != [P, L, P, L, I32, I64, P] {
        return Err(component_error(format!(
            "api shim: stdlib.api-error-json has flat params {:?}",
            stdlib_sig.params
        )));
    }
    if waits {
        let register_ty = api_result("waits", "register")?;
        let register = import("waits.register", &[P, L, P, L, P])?;
        let release = import("waits.release", &[P, L, P])?;
        // `register(key, request, retptr)`.
        let register_index = native(&[I32; 5], true, &mut |shim| {
            shim.call(register, &[0, 1, 2, 3], true);
            shim.on_error_return_json(&register_ty, NATIVE_PAYLOAD, stdlib)?;
            native_list(shim, &register_ty)
        })?;
        // `release(key)` cannot fail: the result is already checkpointed,
        // and a settled wait left behind goes with the instance.
        let release_index = native(&[I32; 2], false, &mut |shim| {
            shim.call(release, &[0, 1], true);
            Ok(())
        })?;
        indices.wait_instances = Some(super::core_imports::DirectWaitInstancesImports {
            register: register_index,
            release: release_index,
        });
    }
    if state {
        let (set_ty, get_ty) = (api_result("state", "set")?, api_result("state", "get")?);
        let set = import("state.set", &[P, L, P, L, P])?;
        let get = import("state.get", &[P, L, P])?;
        // `set(key, patch, retptr)` and `get(key, retptr)`.
        let set_index = native(&[I32; 5], true, &mut |shim| {
            shim.call(set, &[0, 1, 2, 3], true);
            shim.on_error_return_json(&set_ty, NATIVE_PAYLOAD, stdlib)
        })?;
        let get_index = native(&[I32; 3], true, &mut |shim| {
            shim.call(get, &[0, 1], true);
            shim.on_error_return_json(&get_ty, NATIVE_PAYLOAD, stdlib)?;
            native_list(shim, &get_ty)
        })?;
        indices.run_state = Some(super::core_imports::DirectRunStateImports {
            set: set_index,
            get: get_index,
        });
    }
    if child_runs {
        indices.child_run = Some(emit_child_run_shim(
            &layouts,
            &import,
            &api_result,
            &mut native,
            scratch + API_SCRATCH_BYTES + OPERATION_CELL_BYTES,
        )?);
    }
    Ok(())
}

/// Bytes after the operation cell where the `child-run` helper builds its
/// wait request, `{"instanceIds":["<task id>"],"mode":"all","deadlineMs":n}`;
/// the heap starts after them when a child run exists.
pub(super) const CHILD_RUN_REQUEST_BYTES: i32 = 216;
const CHILD_RUN_REQUEST_PREFIX: &[u8] = br#"{"instanceIds":[""#;
const CHILD_RUN_REQUEST_SUFFIX: &[u8] = br#""],"mode":"all""#;
const CHILD_RUN_REQUEST_DEADLINE: &[u8] = br#","deadlineMs":"#;

/// What `child-run` leaves at its result pointer: the tag, then the payload
/// at 8, where the composed Agent invoke leaves its normalized result.
pub(super) mod child_run {
    /// `ok(list<u8>)`: the child's output, ptr/len at 8.
    pub(crate) const COMPLETED: i32 = 0;
    /// `err(error-info)`: the child's error at 8.
    pub(crate) const FAILED: i32 = 1;
    /// Still running: its wait is registered and the run parks.
    pub(crate) const PENDING: i32 = 2;
    pub(crate) const CANCELLED: i32 = 3;
    pub(crate) const TIMED_OUT: i32 = 4;
    pub(crate) const TRAPPED: i32 = 5;
    /// The workflow API refused or failed a call; its message is at
    /// [`MESSAGE`].
    pub(crate) const UNAVAILABLE: i32 = 6;
    /// Where an `unavailable` result keeps the API's message (ptr, len),
    /// past the error-info the step error is written into.
    pub(crate) const MESSAGE: u64 = 96;
    /// The start was refused for capacity, retryably: the run parks and
    /// starts it again after the hint (ms, a `u64` at [`RETRY_AFTER`]).
    pub(crate) const CAPACITY: i32 = 7;
    pub(crate) const RETRY_AFTER: u64 = 8;
}

/// `child-run(binding, input, path, attempt, deadline, settled, retptr)`:
/// start the child run of `binding` for this call site (a replay finds the
/// same child) and read its outcome without waiting. While it still runs,
/// register a wait on it under `path`, with `deadline` (ms since the epoch,
/// 0 for none), and answer `pending` with the wait's read at 8 so the run can
/// park; with `settled` (the wait's deadline passed) cancel it instead and
/// answer `timed-out`. An ended child's task and wait are released.
/// Looks up an imported `runtara:api` function by `interface.function`,
/// checking its core signature.
type ImportLookup<'a> = dyn Fn(&str, &[WasmType]) -> Result<&'a ApiImport, DirectCompileError> + 'a;
/// Defines a helper with these params (its result pointer last when it
/// returns) and the body the callback writes; answers its function index.
type DefineNative<'d> = dyn FnMut(
        &[WasmType],
        bool,
        &mut dyn FnMut(&mut Shim) -> Result<(), DirectCompileError>,
    ) -> Result<u32, DirectCompileError>
    + 'd;

fn emit_child_run_shim<'a>(
    layouts: &Layouts<'_>,
    import: &ImportLookup<'a>,
    api_result: &dyn Fn(&str, &str) -> Result<Type, DirectCompileError>,
    native: &mut DefineNative<'_>,
    request: i32,
) -> Result<u32, DirectCompileError> {
    use wit_parser::abi::WasmType::{I32, I64, Length as L, Pointer as P};
    let start_ty = api_result("tasks", "start")?;
    let outcome_ty = api_result("tasks", "outcome")?;
    let register_ty = api_result("waits", "register")?;
    let message_of = |ty: &Type| layouts.offset(ty, &[Step::Err, Step::Field("message")]);
    let start_code = layouts.offset(&start_ty, &[Step::Err, Step::Field("code")])?;
    let start_retry = layouts.offset(&start_ty, &[Step::Err, Step::Field("retry-after-ms")])?;
    let start_retry_value = layouts.offset(
        &start_ty,
        &[Step::Err, Step::Field("retry-after-ms"), Step::Some],
    )?;
    let (start_message, outcome_message, register_message) = (
        message_of(&start_ty)?,
        message_of(&outcome_ty)?,
        message_of(&register_ty)?,
    );
    let start = import("tasks.start", &[P, L, I32, P, L, P, L, P, L, I64, P])?.index;
    let outcome = import("tasks.outcome", &[P, L, I64, P])?.index;
    let release = import("tasks.release", &[P, L, P])?.index;
    let register = import("waits.register", &[P, L, P, L, P])?.index;
    let wait_release = import("waits.release", &[P, L, P])?.index;
    let id_at = layouts.offset(&start_ty, &[Step::Ok])?;
    let ended_at = layouts.offset(&outcome_ty, &[Step::Ok])?;
    let case_at = layouts.offset(&outcome_ty, &[Step::Ok, Step::Some])?;
    let output_at = layouts.offset(
        &outcome_ty,
        &[Step::Ok, Step::Some, Step::Case("completed")],
    )?;
    let error_at = layouts.offset(&outcome_ty, &[Step::Ok, Step::Some, Step::Case("failed")])?;
    // The composed invoke's `error-info`, which this one copies over.
    const ERROR_INFO_BYTES: i32 = 80;
    let read_at = layouts.offset(&register_ty, &[Step::Ok])?;
    // `"deadlineMs":` takes up to 20 digits, and `}` closes the request.
    let id_max = CHILD_RUN_REQUEST_BYTES
        - (CHILD_RUN_REQUEST_PREFIX.len()
            + CHILD_RUN_REQUEST_SUFFIX.len()
            + CHILD_RUN_REQUEST_DEADLINE.len()
            + 21) as i32;
    native(&[P, L, P, L, P, L, I32, I64, I32, P], true, &mut |shim| {
        let scratch = shim.scratch;
        let retptr = shim.retptr;
        let [id_ptr, id_len, tag] = [0, 1, 2].map(|n| shim.local(n));
        let attempt = shim.tmp_i64;
        // A refused or failed call is `unavailable`, with the API's message.
        let unavailable_if_error = |shim: &mut Shim, message: u64| {
            let (ptr, len) = (shim.local(5), shim.local(6));
            shim.i(Instruction::I32Const(scratch))
                .i(Instruction::I32Load8U(mem(0, 0)))
                .i(Instruction::If(BlockType::Empty));
            shim.load_i32(message, ptr);
            shim.load_i32(message + 4, len);
            shim.store_i32(child_run::MESSAGE, ptr);
            shim.store_i32(child_run::MESSAGE + 4, len);
            shim.store_const_u8(0, child_run::UNAVAILABLE);
            shim.i(Instruction::Return).i(Instruction::End);
        };
        // The task (cancelling a child that still runs) and its wait go.
        let release_both = |shim: &mut Shim| {
            shim.i(Instruction::LocalGet(id_ptr))
                .i(Instruction::LocalGet(id_len))
                .i(Instruction::I32Const(scratch))
                .i(Instruction::Call(release));
            shim.i(Instruction::LocalGet(4))
                .i(Instruction::LocalGet(5))
                .i(Instruction::I32Const(scratch))
                .i(Instruction::Call(wait_release));
        };

        // start(binding, workflow, input, {path, attempt})
        shim.i(Instruction::LocalGet(6))
            .i(Instruction::I64ExtendI32U)
            .i(Instruction::LocalSet(attempt));
        shim.i(Instruction::LocalGet(0))
            .i(Instruction::LocalGet(1))
            .i(Instruction::I32Const(1))
            .i(Instruction::I32Const(0))
            .i(Instruction::I32Const(0));
        for param in [2, 3, 4, 5] {
            shim.i(Instruction::LocalGet(param));
        }
        shim.i(Instruction::LocalGet(attempt))
            .i(Instruction::I32Const(scratch))
            .i(Instruction::Call(start));
        // A full share with a hint is `capacity`: the caller parks and
        // starts again. Without a hint it can never be met: `unavailable`.
        shim.i(Instruction::I32Const(scratch))
            .i(Instruction::I32Load8U(mem(0, 0)))
            .i(Instruction::If(BlockType::Empty));
        let (code_ptr, code_len) = (shim.local(5), shim.local(6));
        shim.load_i32(start_code, code_ptr);
        shim.load_i32(start_code + 4, code_len);
        code_is(shim, code_ptr, code_len, "capacity");
        shim.i(Instruction::I32Const(scratch))
            .i(Instruction::I32Load8U(mem(start_retry, 0)))
            .i(Instruction::I32And)
            .i(Instruction::If(BlockType::Empty))
            .i(Instruction::LocalGet(retptr))
            .i(Instruction::I32Const(scratch))
            .i(Instruction::I64Load(mem(start_retry_value, 3)))
            .i(Instruction::I64Store(mem(child_run::RETRY_AFTER, 3)));
        shim.store_const_u8(0, child_run::CAPACITY);
        shim.i(Instruction::Return)
            .i(Instruction::End)
            .i(Instruction::End);
        unavailable_if_error(shim, start_message);
        shim.load_i32(id_at, id_ptr);
        shim.load_i32(id_at + 4, id_len);

        // outcome(id, 0)
        shim.i(Instruction::LocalGet(id_ptr))
            .i(Instruction::LocalGet(id_len))
            .i(Instruction::I64Const(0))
            .i(Instruction::I32Const(scratch))
            .i(Instruction::Call(outcome));
        unavailable_if_error(shim, outcome_message);

        // Still running. On a settled wait the deadline passed: the child is
        // cancelled and the call timed out.
        shim.load_u8(ended_at, tag);
        shim.i(Instruction::LocalGet(tag))
            .i(Instruction::I32Eqz)
            .i(Instruction::If(BlockType::Empty));
        shim.i(Instruction::LocalGet(8))
            .i(Instruction::If(BlockType::Empty));
        release_both(shim);
        shim.store_const_u8(0, child_run::TIMED_OUT);
        shim.i(Instruction::Return).i(Instruction::End);

        // Otherwise register(path, {"instanceIds":[id],"mode":"all",
        // "deadlineMs"?}) and leave its read for the caller.
        shim.i(Instruction::LocalGet(id_len))
            .i(Instruction::I32Const(id_max))
            .i(Instruction::I32GtU)
            .i(Instruction::If(BlockType::Empty));
        shim.store_i32(child_run::MESSAGE, id_ptr);
        shim.store_i32(child_run::MESSAGE + 4, id_len);
        shim.store_const_u8(0, child_run::UNAVAILABLE);
        shim.i(Instruction::Return).i(Instruction::End);
        let cursor = shim.local(7);
        shim.i(Instruction::I32Const(request))
            .i(Instruction::LocalSet(cursor));
        write_bytes(shim, cursor, CHILD_RUN_REQUEST_PREFIX);
        shim.i(Instruction::LocalGet(cursor))
            .i(Instruction::LocalGet(id_ptr))
            .i(Instruction::LocalGet(id_len))
            .i(Instruction::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            })
            .i(Instruction::LocalGet(cursor))
            .i(Instruction::LocalGet(id_len))
            .i(Instruction::I32Add)
            .i(Instruction::LocalSet(cursor));
        write_bytes(shim, cursor, CHILD_RUN_REQUEST_SUFFIX);
        shim.i(Instruction::LocalGet(7))
            .i(Instruction::I64Eqz)
            .i(Instruction::I32Eqz)
            .i(Instruction::If(BlockType::Empty));
        write_bytes(shim, cursor, CHILD_RUN_REQUEST_DEADLINE);
        write_decimal(shim, cursor, 7);
        shim.i(Instruction::End);
        write_bytes(shim, cursor, b"}");
        shim.i(Instruction::LocalGet(4))
            .i(Instruction::LocalGet(5))
            .i(Instruction::I32Const(request))
            .i(Instruction::LocalGet(cursor))
            .i(Instruction::I32Const(request))
            .i(Instruction::I32Sub)
            .i(Instruction::I32Const(scratch))
            .i(Instruction::Call(register));
        unavailable_if_error(shim, register_message);
        let (ptr, len) = (shim.local(3), shim.local(4));
        shim.load_i32(read_at, ptr);
        shim.load_i32(read_at + 4, len);
        shim.store_i32(8, ptr);
        shim.store_i32(12, len);
        shim.store_const_u8(0, child_run::PENDING);
        shim.i(Instruction::Return).i(Instruction::End);

        // Ended: its outcome, then the task and the wait go.
        shim.load_u8(case_at, tag);
        // completed(list)
        shim.i(Instruction::LocalGet(tag))
            .i(Instruction::I32Eqz)
            .i(Instruction::If(BlockType::Empty));
        let (ptr, len) = (shim.local(3), shim.local(4));
        shim.load_i32(output_at, ptr);
        shim.load_i32(output_at + 4, len);
        shim.store_i32(8, ptr);
        shim.store_i32(12, len);
        shim.i(Instruction::I32Const(child_run::COMPLETED))
            .i(Instruction::LocalSet(tag))
            .i(Instruction::Else);
        // failed(error-info)
        shim.i(Instruction::LocalGet(tag))
            .i(Instruction::I32Const(1))
            .i(Instruction::I32Eq)
            .i(Instruction::If(BlockType::Empty));
        shim.i(Instruction::LocalGet(retptr))
            .i(Instruction::I32Const(8))
            .i(Instruction::I32Add)
            .i(Instruction::I32Const(scratch + error_at as i32))
            .i(Instruction::I32Const(ERROR_INFO_BYTES))
            .i(Instruction::MemoryCopy {
                src_mem: 0,
                dst_mem: 0,
            })
            .i(Instruction::I32Const(child_run::FAILED))
            .i(Instruction::LocalSet(tag))
            .i(Instruction::Else);
        // suspended (never a run of its own) and trapped are a trap; the
        // others keep their case number.
        shim.i(Instruction::LocalGet(tag))
            .i(Instruction::I32Const(2))
            .i(Instruction::I32Eq)
            .i(Instruction::If(BlockType::Result(ValType::I32)))
            .i(Instruction::I32Const(child_run::TRAPPED))
            .i(Instruction::Else)
            .i(Instruction::LocalGet(tag))
            .i(Instruction::End)
            .i(Instruction::LocalSet(tag))
            .i(Instruction::End)
            .i(Instruction::End);
        release_both(shim);
        shim.store_u8(0, tag);
        shim.i(Instruction::Return);
        Ok(())
    })
}

/// Store `bytes` at `cursor`, advancing it.
fn write_bytes(shim: &mut Shim, cursor: u32, bytes: &[u8]) {
    for (n, byte) in bytes.iter().enumerate() {
        shim.i(Instruction::LocalGet(cursor))
            .i(Instruction::I32Const(i32::from(*byte)))
            .i(Instruction::I32Store8(mem(n as u64, 0)));
    }
    shim.i(Instruction::LocalGet(cursor))
        .i(Instruction::I32Const(bytes.len() as i32))
        .i(Instruction::I32Add)
        .i(Instruction::LocalSet(cursor));
}

/// Store the i64 `value` local (consumed) in decimal at `cursor`, advancing
/// it.
fn write_decimal(shim: &mut Shim, cursor: u32, value: u32) {
    let rest = shim.tmp_i64;
    // Advance past the digits, then write them backwards.
    shim.i(Instruction::LocalGet(value))
        .i(Instruction::LocalSet(rest))
        .i(Instruction::Loop(BlockType::Empty))
        .i(Instruction::LocalGet(cursor))
        .i(Instruction::I32Const(1))
        .i(Instruction::I32Add)
        .i(Instruction::LocalSet(cursor))
        .i(Instruction::LocalGet(rest))
        .i(Instruction::I64Const(10))
        .i(Instruction::I64DivU)
        .i(Instruction::LocalTee(rest))
        .i(Instruction::I64Eqz)
        .i(Instruction::I32Eqz)
        .i(Instruction::BrIf(0))
        .i(Instruction::End);
    let at = shim.local(8);
    shim.i(Instruction::LocalGet(cursor))
        .i(Instruction::LocalSet(at))
        .i(Instruction::Loop(BlockType::Empty))
        .i(Instruction::LocalGet(at))
        .i(Instruction::I32Const(1))
        .i(Instruction::I32Sub)
        .i(Instruction::LocalTee(at))
        .i(Instruction::LocalGet(value))
        .i(Instruction::I64Const(10))
        .i(Instruction::I64RemU)
        .i(Instruction::I32WrapI64)
        .i(Instruction::I32Const(i32::from(b'0')))
        .i(Instruction::I32Add)
        .i(Instruction::I32Store8(mem(0, 0)))
        .i(Instruction::LocalGet(value))
        .i(Instruction::I64Const(10))
        .i(Instruction::I64DivU)
        .i(Instruction::LocalTee(value))
        .i(Instruction::I64Eqz)
        .i(Instruction::I32Eqz)
        .i(Instruction::BrIf(0))
        .i(Instruction::End);
}

fn set_cell(shim: &mut Shim, cell: i32, offset: u64, value: i32) {
    shim.i(Instruction::I32Const(cell))
        .i(Instruction::I32Const(value))
        .i(Instruction::I32Store(mem(offset, 2)));
}

/// Push the operation cell's state.
fn cell_state(shim: &mut Shim, cell: i32) {
    shim.i(Instruction::I32Const(cell))
        .i(Instruction::I32Load(mem(OPERATION_STATE, 2)));
}

/// Push whether the string at `ptr`/`len` is `text`; its bytes are read only
/// when the length matches.
fn code_is(shim: &mut Shim, ptr: u32, len: u32, text: &str) {
    shim.i(Instruction::LocalGet(len))
        .i(Instruction::I32Const(text.len() as i32))
        .i(Instruction::I32Eq)
        .i(Instruction::If(BlockType::Result(ValType::I32)))
        .i(Instruction::I32Const(1));
    for (n, byte) in text.bytes().enumerate() {
        shim.i(Instruction::LocalGet(ptr))
            .i(Instruction::I32Load8U(mem(n as u64, 0)))
            .i(Instruction::I32Const(i32::from(byte)))
            .i(Instruction::I32Eq)
            .i(Instruction::I32And);
    }
    shim.i(Instruction::Else)
        .i(Instruction::I32Const(0))
        .i(Instruction::End);
}

/// `option<list<u8>>` from `new`'s ok payload into `old`'s.
fn copy_option_list(shim: &mut Shim, new: &Type) -> Result<(), DirectCompileError> {
    let layouts = shim.layouts;
    let (from, from_list) = (
        layouts.offset(new, &[Step::Ok])?,
        layouts.offset(new, &[Step::Ok, Step::Some])?,
    );
    let (tag, ptr, len) = (shim.local(2), shim.local(3), shim.local(4));
    shim.load_u8(from, tag);
    shim.load_i32(from_list, ptr);
    shim.load_i32(from_list + 4, len);
    shim.store_u8(native::PAYLOAD, tag);
    shim.store_i32(native::OPTION_LIST, ptr);
    shim.store_i32(native::OPTION_LIST + 4, len);
    Ok(())
}

/// `option<lifecycle-signal>` (new) as `option<signal-info>` (old).
fn write_polled_signal(shim: &mut Shim, new: &Type) -> Result<(), DirectCompileError> {
    use Step::{Field, Ok, Some};
    let layouts = shim.layouts;
    // (new path, old path) of each string or list field.
    let fields: [(&[Step], u64); 3] = [
        (
            &[Ok, Some, Field("notice"), Field("signal-type")],
            native::SIGNAL_TYPE,
        ),
        (
            &[Ok, Some, Field("notice"), Field("command-id")],
            native::SIGNAL_COMMAND,
        ),
        (&[Ok, Some, Field("payload")], native::SIGNAL_PAYLOAD),
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

    let base = native::POLLED_SIGNAL;
    shim.store_u8(native::PAYLOAD, tag);
    for (n, (_, to)) in fields.iter().enumerate() {
        let (ptr, len) = loaded[n];
        shim.store_i32(base + to, ptr);
        shim.store_i32(base + to + 4, len);
    }
    shim.store_u8(base + native::SIGNAL_CHECKPOINT, cp_tag);
    shim.store_i32(base + native::SIGNAL_CHECKPOINT_VALUE, cp_ptr);
    shim.store_i32(base + native::SIGNAL_CHECKPOINT_VALUE + 4, cp_len);
    Result::Ok(())
}

/// `checkpoint-written` (new) as the old `checkpoint-result`. Call sites read
/// `found`, `state` and the pending signal's type and command id; the old
/// result's signal payload and custom signal are left empty.
fn write_checkpoint_result(shim: &mut Shim, new: &Type) -> Result<(), DirectCompileError> {
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

    shim.store_u8(native::CHECKPOINT_FOUND, found);
    shim.store_i32(native::CHECKPOINT_STATE, state_ptr);
    shim.store_i32(native::CHECKPOINT_STATE + 4, state_len);
    shim.store_u8(native::CHECKPOINT_PENDING, tag);
    let signal = native::CHECKPOINT_SIGNAL;
    for (to, ptr, len) in [
        (native::SIGNAL_TYPE, ty_ptr, ty_len),
        (native::SIGNAL_COMMAND, cmd_ptr, cmd_len),
    ] {
        shim.store_i32(signal + to, ptr);
        shim.store_i32(signal + to + 4, len);
    }
    shim.store_const_i32(signal + native::SIGNAL_PAYLOAD, 0);
    shim.store_const_i32(signal + native::SIGNAL_PAYLOAD + 4, 0);
    shim.store_const_u8(signal + native::SIGNAL_CHECKPOINT, 0);
    shim.store_const_u8(native::CHECKPOINT_CUSTOM, 0);
    Ok(())
}

/// Where `runtara:api/connections.describe` leaves its answer, and where the
/// logic reads it: the emitter's own `result<list<u8>, string>`, payload at 4.
/// The connection lookup is async-lowered, so it cannot go through a helper:
/// once the call resolves, its answer is moved into that layout in place, the
/// api-error's message standing for the error string (a connection failure's
/// message is the host's own text).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DescribeMoves {
    /// The descriptor `list<u8>` of the api answer.
    pub ok_from: u64,
    /// The api-error's `message` string.
    pub err_from: u64,
    /// The logic's payload: the descriptor, or the error string.
    pub to: u64,
}

pub(super) fn describe_moves(resolve: &Resolve) -> Result<DescribeMoves, DirectCompileError> {
    let layouts = Layouts::new(resolve);
    let new = result_type(function(resolve, "runtara:api", "connections", "describe")?)?;
    let moves = DescribeMoves {
        ok_from: layouts.offset(&new, &[Step::Ok])?,
        err_from: layouts.offset(&new, &[Step::Err, Step::Field("message")])?,
        to: 4,
    };
    // Both are moved down word by word, so a source never sits below the
    // destination.
    if moves.ok_from < moves.to || moves.err_from < moves.to {
        return Err(component_error(
            "api shim: describe answers cannot be moved in place",
        ));
    }
    Ok(moves)
}

/// Byte offset of `path` in the result of `runtara:api` `interface.name`,
/// for the tests that answer the shims' imports.
#[cfg(test)]
pub(super) fn api_result_offset(
    resolve: &Resolve,
    interface: &str,
    name: &str,
    path: &[Step],
) -> u64 {
    let ty = result_type(function(resolve, "runtara:api", interface, name).unwrap()).unwrap();
    Layouts::new(resolve).offset(&ty, path).unwrap()
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
            let new_ty =
                result_type(function(&resolve, "runtara:api", new.0, new.1).unwrap()).unwrap();
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
    fn describe_answers_move_down_into_the_old_layout() {
        let resolve = resolve();
        let moves = describe_moves(&resolve).unwrap();
        assert_eq!(
            moves,
            DescribeMoves {
                ok_from: 8,
                err_from: 16,
                to: 4
            }
        );
        let layouts = Layouts::new(&resolve);
        let new =
            result_type(function(&resolve, "runtara:api", "connections", "describe").unwrap())
                .unwrap();
        // It is written where an Agent's result goes, which has 120 bytes.
        assert!(layouts.size(&new) <= 120);
    }

    /// The helpers write the layouts the call sites read.
    #[test]
    fn helper_layouts_are_the_call_sites_constants() {
        use super::super::*;
        assert_eq!(native::CHECKPOINT_FOUND, DIRECT_CHECKPOINT_FOUND_OFFSET);
        assert_eq!(
            native::CHECKPOINT_PENDING,
            DIRECT_CHECKPOINT_PENDING_SIGNAL_TAG_OFFSET
        );
        assert_eq!(
            native::CHECKPOINT_SIGNAL + native::SIGNAL_TYPE,
            DIRECT_CHECKPOINT_SIGNAL_TYPE_PTR_OFFSET
        );
        assert_eq!(native::PAYLOAD, DIRECT_RESULT_OPTION_TAG_OFFSET);
        assert_eq!(native::OPTION_LIST, DIRECT_RESULT_OPTION_LIST_PTR_OFFSET);
        assert_eq!(native::U64_PAYLOAD, DIRECT_RET_U64_OK_OFFSET);
    }

    #[test]
    fn api_offsets_follow_the_wit() {
        let resolve = resolve();
        let layouts = Layouts::new(&resolve);
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
