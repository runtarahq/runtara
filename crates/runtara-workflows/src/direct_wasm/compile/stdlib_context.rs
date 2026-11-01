// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Threads the stdlib context through a workflow logic core module.
//!
//! One stdlib instance can serve several logic instances (a run's root and
//! each published workflow-agent it composes), so every stdlib call names the
//! caller's context: `init-manifest` takes the caller's previous context and
//! returns the one to use, and every other function takes it first.
//!
//! The emitter writes stdlib calls without the context. This pass adds an
//! i64 `ctx` global (initially -1, `u64::MAX`: no context yet), one shim per stdlib import
//! that pushes the global ahead of its own parameters and calls the import,
//! and routes every call through its shim. The `init-manifest` shim also
//! stores the context it returns. Types, functions and the global are
//! appended, so no existing index moves.
//!
//! Two functions flatten past 16 values and take their arguments by pointer.
//! `agent-retry-error-info` is pure and takes no context. `agent-error`'s
//! arguments are the Agent's result area, `agent-id @0, error-info @8`; its
//! context comes last, at [`AGENT_ERROR_CONTEXT_OFFSET`], so its shim writes
//! the context there and leaves the record where the Agent wrote it.

use wasm_encoder::reencode::{Reencode, utils};
use wasm_encoder::{ConstExpr, Function, GlobalType, Instruction, MemArg, ValType};

use super::{DirectCompileError, component_error};

const INIT_MANIFEST: &str = "init-manifest";
const AGENT_ERROR: &str = "agent-error";
/// Where `agent-error`'s trailing context sits in its argument area: after
/// `agent-id: u32` and the 80-byte, 8-aligned `error-info` at 8.
pub(super) const AGENT_ERROR_CONTEXT_OFFSET: u64 = 88;
/// Stdlib functions without a context parameter.
const CONTEXT_FREE: &[&str] = &["agent-retry-error-info"];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shim {
    /// Push the context ahead of the shim's own parameters.
    Prepend,
    /// [`Shim::Prepend`], then keep the context `init-manifest` returns.
    Init,
    /// Write the context into the by-pointer argument area (`agent-error`).
    Area,
}

/// Whether a core import module is the stdlib's (`cm32p2|runtara:workflow-stdlib/json@1`
/// and the linked variant).
pub(super) fn is_stdlib_module(module: &str) -> bool {
    let name = module.rsplit('|').next().unwrap_or(module);
    name.starts_with("runtara:workflow-stdlib")
}

struct StdlibImport {
    index: u32,
    /// The shim's own parameters.
    params: Vec<ValType>,
    results: Vec<ValType>,
    shim: Shim,
}

/// Add the context global and shims to `core`; unchanged when it imports no
/// stdlib function.
pub(super) fn thread_stdlib_context(core: &[u8]) -> Result<Vec<u8>, DirectCompileError> {
    let wrap = |e: wasmparser::BinaryReaderError| component_error(e.to_string());
    let mut types: Vec<(Vec<ValType>, Vec<ValType>)> = Vec::new();
    let mut stdlib = Vec::new();
    let mut imported_functions = 0;
    let mut imported_globals = 0;
    let mut defined_functions = 0;
    let mut defined_globals = 0;
    for payload in wasmparser::Parser::new(0).parse_all(core) {
        match payload.map_err(wrap)? {
            wasmparser::Payload::TypeSection(reader) => {
                for ty in reader.into_iter_err_on_gc_types() {
                    let ty = ty.map_err(wrap)?;
                    let val = |t: &wasmparser::ValType| val_type(*t);
                    types.push((
                        ty.params().iter().map(val).collect::<Result<_, _>>()?,
                        ty.results().iter().map(val).collect::<Result<_, _>>()?,
                    ));
                }
            }
            wasmparser::Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.map_err(wrap)?;
                    match import.ty {
                        wasmparser::TypeRef::Func(ty) => {
                            if is_stdlib_module(import.module)
                                && !CONTEXT_FREE.contains(&import.name)
                            {
                                let (params, results) = types
                                    .get(ty as usize)
                                    .cloned()
                                    .ok_or_else(|| component_error("stdlib import type"))?;
                                let shim = match import.name {
                                    INIT_MANIFEST => Shim::Init,
                                    AGENT_ERROR => Shim::Area,
                                    _ => Shim::Prepend,
                                };
                                let own = match shim {
                                    // (args pointer, return pointer), unchanged.
                                    Shim::Area if params == [ValType::I32, ValType::I32] => params,
                                    Shim::Prepend | Shim::Init
                                        if params.first() == Some(&ValType::I64) =>
                                    {
                                        params[1..].to_vec()
                                    }
                                    _ => {
                                        return Err(component_error(format!(
                                            "stdlib import `{}` has an unexpected signature {params:?}",
                                            import.name
                                        )));
                                    }
                                };
                                stdlib.push(StdlibImport {
                                    index: imported_functions,
                                    params: own,
                                    results,
                                    shim,
                                });
                            }
                            imported_functions += 1;
                        }
                        wasmparser::TypeRef::Global(_) => imported_globals += 1,
                        _ => {}
                    }
                }
            }
            wasmparser::Payload::FunctionSection(reader) => defined_functions = reader.count(),
            wasmparser::Payload::GlobalSection(reader) => defined_globals = reader.count(),
            _ => {}
        }
    }
    if stdlib.is_empty() {
        return Ok(core.to_vec());
    }
    let mut pass = Pass {
        ctx_global: imported_globals + defined_globals,
        first_shim: imported_functions + defined_functions,
        first_shim_type: types.len() as u32,
        stdlib,
    };
    let mut out = wasm_encoder::Module::new();
    pass.parse_core_module(&mut out, wasmparser::Parser::new(0), core)
        .map_err(|e| component_error(format!("stdlib context: {e:?}")))?;
    Ok(out.finish())
}

/// Each context-taking stdlib import with the shim that stands for it, for
/// tests that read the emitted module: the emitter's calls reach the imports
/// through these.
#[cfg(test)]
pub(super) fn shims(core: &[u8]) -> Vec<(u32, u32)> {
    let mut imported = 0;
    let mut defined = 0;
    let mut stdlib = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(core) {
        match payload.expect("core payload") {
            wasmparser::Payload::ImportSection(reader) => {
                for import in reader.into_imports() {
                    let import = import.expect("import");
                    if let wasmparser::TypeRef::Func(_) = import.ty {
                        if is_stdlib_module(import.module) && !CONTEXT_FREE.contains(&import.name) {
                            stdlib.push(imported);
                        }
                        imported += 1;
                    }
                }
            }
            wasmparser::Payload::FunctionSection(reader) => defined = reader.count(),
            _ => {}
        }
    }
    let first_shim = imported + defined - stdlib.len() as u32;
    stdlib
        .into_iter()
        .enumerate()
        .map(|(position, import)| (import, first_shim + position as u32))
        .collect()
}

/// Whether `name`, a stdlib import, takes the context as its first core
/// parameter (`agent-error` takes it in its argument area).
#[cfg(test)]
pub(super) fn takes_context(name: &str) -> bool {
    !CONTEXT_FREE.contains(&name) && name != AGENT_ERROR
}

fn val_type(ty: wasmparser::ValType) -> Result<ValType, DirectCompileError> {
    Ok(match ty {
        wasmparser::ValType::I32 => ValType::I32,
        wasmparser::ValType::I64 => ValType::I64,
        wasmparser::ValType::F32 => ValType::F32,
        wasmparser::ValType::F64 => ValType::F64,
        other => {
            return Err(component_error(format!(
                "stdlib context: unexpected value type {other:?}"
            )));
        }
    })
}

struct Pass {
    ctx_global: u32,
    first_shim: u32,
    first_shim_type: u32,
    stdlib: Vec<StdlibImport>,
}

impl Pass {
    fn shim_body(&self, import: &StdlibImport) -> Function {
        let own = &import.params;
        let mut body = Function::new([]);
        if import.shim == Shim::Area {
            body.instruction(&Instruction::LocalGet(0));
            body.instruction(&Instruction::GlobalGet(self.ctx_global));
            body.instruction(&Instruction::I64Store(MemArg {
                offset: AGENT_ERROR_CONTEXT_OFFSET,
                align: 3,
                memory_index: 0,
            }));
        } else {
            body.instruction(&Instruction::GlobalGet(self.ctx_global));
        }
        for local in 0..own.len() as u32 {
            body.instruction(&Instruction::LocalGet(local));
        }
        body.instruction(&Instruction::Call(import.index));
        if import.shim == Shim::Init {
            // `result<u64, string>` at the return pointer (the last
            // parameter): tag byte at 0, the context at 8 when it is `ok`.
            let retptr = own.len() as u32 - 1;
            body.instruction(&Instruction::LocalGet(retptr));
            body.instruction(&Instruction::I32Load8U(MemArg {
                offset: 0,
                align: 0,
                memory_index: 0,
            }));
            body.instruction(&Instruction::I32Eqz);
            body.instruction(&Instruction::If(wasm_encoder::BlockType::Empty));
            body.instruction(&Instruction::LocalGet(retptr));
            body.instruction(&Instruction::I64Load(MemArg {
                offset: 8,
                align: 3,
                memory_index: 0,
            }));
            body.instruction(&Instruction::GlobalSet(self.ctx_global));
            body.instruction(&Instruction::End);
        }
        body.instruction(&Instruction::End);
        body
    }
}

impl Reencode for Pass {
    type Error = std::convert::Infallible;

    fn function_index(&mut self, func: u32) -> Result<u32, wasm_encoder::reencode::Error> {
        Ok(self
            .stdlib
            .iter()
            .position(|import| import.index == func)
            .map_or(func, |shim| self.first_shim + shim as u32))
    }

    fn parse_type_section(
        &mut self,
        types: &mut wasm_encoder::TypeSection,
        section: wasmparser::TypeSectionReader<'_>,
    ) -> Result<(), wasm_encoder::reencode::Error> {
        utils::parse_type_section(self, types, section)?;
        for import in &self.stdlib {
            types.ty().function(
                import.params.iter().copied(),
                import.results.iter().copied(),
            );
        }
        Ok(())
    }

    fn parse_function_section(
        &mut self,
        functions: &mut wasm_encoder::FunctionSection,
        section: wasmparser::FunctionSectionReader<'_>,
    ) -> Result<(), wasm_encoder::reencode::Error> {
        utils::parse_function_section(self, functions, section)?;
        for shim in 0..self.stdlib.len() as u32 {
            functions.function(self.first_shim_type + shim);
        }
        Ok(())
    }

    fn parse_global_section(
        &mut self,
        globals: &mut wasm_encoder::GlobalSection,
        section: wasmparser::GlobalSectionReader<'_>,
    ) -> Result<(), wasm_encoder::reencode::Error> {
        utils::parse_global_section(self, globals, section)?;
        globals.global(
            GlobalType {
                val_type: ValType::I64,
                mutable: true,
                shared: false,
            },
            &ConstExpr::i64_const(-1),
        );
        Ok(())
    }

    fn parse_code_section(
        &mut self,
        code: &mut wasm_encoder::CodeSection,
        section: wasmparser::CodeSectionReader<'_>,
    ) -> Result<(), wasm_encoder::reencode::Error> {
        utils::parse_code_section(self, code, section)?;
        for import in &self.stdlib {
            code.function(&self.shim_body(import));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_stdlib_modules() {
        assert!(is_stdlib_module("cm32p2|runtara:workflow-stdlib/json@1"));
        assert!(is_stdlib_module(
            "cm32p2|runtara:workflow-stdlib-link/json@1"
        ));
        assert!(!is_stdlib_module("cm32p2|runtara:api/run@1"));
        assert!(!is_stdlib_module("$root"));
    }

    #[test]
    fn a_module_without_stdlib_imports_is_unchanged() {
        let core = wat::parse_str(r#"(module (global (mut i32) (i32.const 8)))"#).unwrap();
        assert_eq!(thread_stdlib_context(&core).unwrap(), core);
    }

    /// The shim writes `agent-error`'s context where the canonical ABI lays
    /// out its last argument.
    #[test]
    fn agent_error_context_offset_matches_the_wit() {
        let resolve = runtara_wit::resolve().unwrap();
        let interface = resolve
            .interfaces
            .iter()
            .find(|(id, _)| resolve.id_of(*id).as_deref() == Some(runtara_wit::stdlib::JSON))
            .map(|(_, interface)| interface)
            .unwrap();
        let function = &interface.functions[AGENT_ERROR];
        let mut sizes = wit_parser::SizeAlign::default();
        sizes.fill(&resolve);
        let offsets = sizes.field_offsets(function.params.iter().map(|param| &param.ty));
        let (offset, _) = offsets.last().unwrap();
        assert_eq!(offset.size_wasm32() as u64, AGENT_ERROR_CONTEXT_OFFSET);
    }

    #[test]
    fn by_pointer_imports_keep_their_signature() {
        let core = wat::parse_str(
            r#"(module
                (import "cm32p2|runtara:workflow-stdlib/json@1" "agent-error"
                    (func $error (param i32 i32)))
                (import "cm32p2|runtara:workflow-stdlib/json@1" "agent-retry-error-info"
                    (func $retry (param i32 i32)))
                (memory 1)
                (global $heap (mut i32) (i32.const 1024))
                (func (export "run")
                    i32.const 64 i32.const 64 call $error
                    i32.const 64 i32.const 64 call $retry))"#,
        )
        .unwrap();
        let threaded = thread_stdlib_context(&core).unwrap();
        wasmparser::Validator::new()
            .validate_all(&threaded)
            .expect("threaded module validates");
        let mut calls = Vec::new();
        for payload in wasmparser::Parser::new(0).parse_all(&threaded) {
            if let wasmparser::Payload::CodeSectionEntry(body) = payload.unwrap() {
                let mut ops = Vec::new();
                for op in body.get_operators_reader().unwrap() {
                    if let wasmparser::Operator::Call { function_index } = op.unwrap() {
                        ops.push(function_index);
                    }
                }
                calls.push(ops);
            }
        }
        // `agent-error` goes through its shim (function 3); the context-free
        // `agent-retry-error-info` stays a direct call.
        assert_eq!(calls, vec![vec![3, 1], vec![0]]);
    }

    #[test]
    fn calls_go_through_shims_that_pass_the_context() {
        let core = wat::parse_str(
            r#"(module
                (import "cm32p2|runtara:workflow-stdlib/json@1" "init-manifest"
                    (func $init (param i64 i32 i32 i32)))
                (import "cm32p2|runtara:workflow-stdlib/json@1" "value-store-scope"
                    (func $scope (param i64) (result i64)))
                (memory 1)
                (global $heap (mut i32) (i32.const 1024))
                (func (export "run") (result i64)
                    i32.const 0 i32.const 0 i32.const 16
                    call $init
                    call $scope))"#,
        )
        .unwrap();
        let threaded = thread_stdlib_context(&core).unwrap();
        wasmparser::Validator::new()
            .validate_all(&threaded)
            .expect("threaded module validates");

        let mut globals = Vec::new();
        let mut bodies = Vec::new();
        for payload in wasmparser::Parser::new(0).parse_all(&threaded) {
            match payload.unwrap() {
                wasmparser::Payload::GlobalSection(reader) => {
                    for global in reader {
                        let mut ops = global.unwrap().init_expr.get_operators_reader();
                        globals.push(format!("{:?}", ops.read().unwrap()));
                    }
                }
                wasmparser::Payload::CodeSectionEntry(body) => {
                    let mut calls = Vec::new();
                    let mut sets = Vec::new();
                    for op in body.get_operators_reader().unwrap() {
                        match op.unwrap() {
                            wasmparser::Operator::Call { function_index } => {
                                calls.push(function_index)
                            }
                            wasmparser::Operator::GlobalSet { global_index } => {
                                sets.push(global_index)
                            }
                            _ => {}
                        }
                    }
                    bodies.push((calls, sets));
                }
                _ => {}
            }
        }
        // The context global starts at -1, after the heap pointer.
        assert_eq!(globals.len(), 2);
        assert!(globals[1].contains("-1"), "{globals:?}");
        // `run` calls the shims (functions 3 and 4), never the imports.
        assert_eq!(bodies[0].0, vec![3, 4]);
        // The init shim calls `init-manifest` and stores the context; the
        // other shim only forwards.
        assert_eq!(bodies[1], (vec![0], vec![1]));
        assert_eq!(bodies[2], (vec![1], vec![]));
    }
}
