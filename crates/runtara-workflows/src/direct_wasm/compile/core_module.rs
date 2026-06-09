// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Direct core Wasm module assembly and export wiring.
//!
//! Plays the role `rustc` + the linker would in the generated path. `emit_direct_core_module`
//! emits the complete module: types, imports, the single real `wasi:cli/run` body
//! (`direct_run_function` — init manifest, load input, build the initial source,
//! lower the whole run plan, complete), zero-return stubs for the other exports,
//! the Canonical-ABI-mandated realloc/initialize/post-return intrinsics, one linear
//! memory sized to the static-data layout, the seeded heap-base global, and the
//! data segments. The shape must match exactly what `wac compose` expects, while
//! all real logic stays in the one `run` body.

use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, ExportKind, ExportSection,
    Function as WasmFunction, FunctionSection, GlobalSection, GlobalType, ImportSection,
    Instruction, MemorySection, MemoryType, Module, TypeSection, ValType,
};
use wit_parser::abi::WasmType;
use wit_parser::{
    Function as WitFunction, ManglingAndAbi, Resolve, WasmExport, WasmExportKind, WorldId,
    WorldItem, WorldKey,
};

use super::abi::{
    emit_fail_if_retptr_error, load_retptr_list, load_retptr_tag, push_core_type, push_retptr_arg,
    push_segment_args, zero_return_function,
};
use super::core_imports::{
    DirectCoreFunctionIndices, DirectCoreImportIndices, import_core_function,
    is_wasi_cli_run_export,
};
use super::dispatcher::emit_run_plan_mapping;
use super::mapping::emit_build_source;
use super::{
    DIRECT_EMPTY_STEPS_CONTEXT, DirectCompileError, DirectCoreStaticData, DirectDataSegment,
    DirectRunPlan, DirectWorkflowManifest, WASM_PAGE_SIZE, direct_core_variables_json,
    direct_run_plan,
};

#[derive(Debug, Clone)]
pub(super) struct DirectCoreConfig {
    pub(super) run_plan: DirectRunPlan,
    pub(super) static_data: DirectCoreStaticData,
    pub(super) track_events: bool,
}

impl DirectCoreConfig {
    #[cfg(test)]
    pub(super) fn new(
        manifest: &DirectWorkflowManifest,
        manifest_json: &[u8],
        track_events: bool,
    ) -> Result<Self, DirectCompileError> {
        // Test/internal entry: no pre-resolved connection integration ids.
        Self::new_inner(
            manifest,
            manifest_json,
            track_events,
            None,
            &std::collections::HashMap::new(),
        )
    }

    pub(super) fn new_with_workflow_id(
        manifest: &DirectWorkflowManifest,
        manifest_json: &[u8],
        track_events: bool,
        workflow_id: &str,
        connection_integration_ids: &std::collections::HashMap<String, String>,
    ) -> Result<Self, DirectCompileError> {
        Self::new_inner(
            manifest,
            manifest_json,
            track_events,
            Some(workflow_id),
            connection_integration_ids,
        )
    }

    fn new_inner(
        manifest: &DirectWorkflowManifest,
        manifest_json: &[u8],
        track_events: bool,
        workflow_id: Option<&str>,
        connection_integration_ids: &std::collections::HashMap<String, String>,
    ) -> Result<Self, DirectCompileError> {
        let variables_json = direct_core_variables_json(&manifest.graph.variables, workflow_id)?;
        Ok(Self {
            run_plan: direct_run_plan(manifest)?,
            static_data: DirectCoreStaticData::new_with_child_workflows(
                &manifest.graph,
                &manifest.child_workflows,
                manifest_json,
                &variables_json,
                DIRECT_EMPTY_STEPS_CONTEXT,
                connection_integration_ids,
            )?,
            track_events,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum DirectVariables<'a> {
    Segment(&'a DirectDataSegment),
    Locals { ptr_local: u32, len_local: u32 },
}

pub(super) fn emit_direct_core_module(
    resolve: &Resolve,
    world: WorldId,
    config: &DirectCoreConfig,
) -> Result<Vec<u8>, DirectCompileError> {
    let mangling = ManglingAndAbi::Standard32;
    let world = &resolve.worlds[world];

    let mut types = TypeSection::new();
    let mut type_count = 0;
    let mut imports = ImportSection::new();
    let mut imported_function_count = 0;
    let mut import_indices = DirectCoreImportIndices::default();
    let mut functions = FunctionSection::new();
    let mut exports = ExportSection::new();
    let mut code = CodeSection::new();
    let mut next_defined_function = 0;

    for (name, import) in &world.imports {
        match import {
            WorldItem::Function(function) => {
                import_core_function(
                    resolve,
                    mangling,
                    None,
                    function,
                    imported_function_count,
                    &mut types,
                    &mut type_count,
                    &mut imports,
                    &mut import_indices,
                );
                imported_function_count += 1;
            }
            WorldItem::Interface { id, .. } => {
                for function in resolve.interfaces[*id].functions.values() {
                    import_core_function(
                        resolve,
                        mangling,
                        Some(name),
                        function,
                        imported_function_count,
                        &mut types,
                        &mut type_count,
                        &mut imports,
                        &mut import_indices,
                    );
                    imported_function_count += 1;
                }
            }
            WorldItem::Type { .. } => {}
        }
    }

    let import_indices = import_indices.require_all()?;

    for (name, export) in &world.exports {
        match export {
            WorldItem::Function(function) => {
                export_core_function(
                    resolve,
                    mangling,
                    None,
                    function,
                    &mut types,
                    &mut type_count,
                    &mut functions,
                    &mut exports,
                    &mut code,
                    imported_function_count,
                    &mut next_defined_function,
                    &import_indices,
                    config,
                );
            }
            WorldItem::Interface { id, .. } => {
                for function in resolve.interfaces[*id].functions.values() {
                    export_core_function(
                        resolve,
                        mangling,
                        Some(name),
                        function,
                        &mut types,
                        &mut type_count,
                        &mut functions,
                        &mut exports,
                        &mut code,
                        imported_function_count,
                        &mut next_defined_function,
                        &import_indices,
                        config,
                    );
                }
            }
            WorldItem::Type { .. } => {}
        }
    }

    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: config.static_data.memory_min_pages,
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    let memory_name = resolve.wasm_export_name(mangling, WasmExport::Memory);
    exports.export(&memory_name, ExportKind::Memory, 0);

    let mut globals = GlobalSection::new();
    globals.global(
        GlobalType {
            val_type: ValType::I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(config.static_data.heap_base),
    );

    export_realloc(
        resolve,
        mangling,
        &mut types,
        &mut type_count,
        &mut functions,
        &mut exports,
        &mut code,
        imported_function_count,
        &mut next_defined_function,
    );
    export_initialize(
        resolve,
        mangling,
        &mut types,
        &mut type_count,
        &mut functions,
        &mut exports,
        &mut code,
        imported_function_count,
        &mut next_defined_function,
    );

    let mut data = DataSection::new();
    for segment in config.static_data.data_segments() {
        data.active(
            0,
            &ConstExpr::i32_const(segment.offset),
            segment.data.iter().copied(),
        );
    }

    let mut module = Module::new();
    module.section(&types);
    if !imports.is_empty() {
        module.section(&imports);
    }
    module.section(&functions);
    module.section(&memories);
    module.section(&globals);
    module.section(&exports);
    module.section(&code);
    module.section(&data);
    Ok(module.finish())
}

#[allow(clippy::too_many_arguments)]
fn export_core_function(
    resolve: &Resolve,
    mangling: ManglingAndAbi,
    interface: Option<&WorldKey>,
    function: &WitFunction,
    types: &mut TypeSection,
    type_count: &mut u32,
    functions: &mut FunctionSection,
    exports: &mut ExportSection,
    code: &mut CodeSection,
    imported_function_count: u32,
    next_defined_function: &mut u32,
    import_indices: &DirectCoreFunctionIndices,
    config: &DirectCoreConfig,
) {
    let signature = resolve.wasm_signature(mangling.export_variant(), function);
    let type_index = push_core_type(types, type_count, &signature.params, &signature.results);
    functions.function(type_index);
    let function_index = imported_function_count + *next_defined_function;
    *next_defined_function += 1;

    let export_name = resolve.wasm_export_name(
        mangling,
        WasmExport::Func {
            interface,
            func: function,
            kind: WasmExportKind::Normal,
        },
    );
    exports.export(&export_name, ExportKind::Func, function_index);

    let body = if is_wasi_cli_run_export(resolve, interface, function) {
        direct_run_function(import_indices, config)
    } else {
        zero_return_function(&signature.results)
    };
    code.function(&body);

    let post_return_type = push_core_type(types, type_count, &signature.results, &[]);
    functions.function(post_return_type);
    let post_return_index = imported_function_count + *next_defined_function;
    *next_defined_function += 1;

    let post_return_name = resolve.wasm_export_name(
        mangling,
        WasmExport::Func {
            interface,
            func: function,
            kind: WasmExportKind::PostReturn,
        },
    );
    exports.export(&post_return_name, ExportKind::Func, post_return_index);

    let mut post_return = WasmFunction::new([]);
    post_return.instruction(&Instruction::End);
    code.function(&post_return);
}

#[allow(clippy::too_many_arguments)]
fn export_realloc(
    resolve: &Resolve,
    mangling: ManglingAndAbi,
    types: &mut TypeSection,
    type_count: &mut u32,
    functions: &mut FunctionSection,
    exports: &mut ExportSection,
    code: &mut CodeSection,
    imported_function_count: u32,
    next_defined_function: &mut u32,
) {
    let type_index = push_core_type(
        types,
        type_count,
        &[WasmType::I32, WasmType::I32, WasmType::I32, WasmType::I32],
        &[WasmType::I32],
    );
    functions.function(type_index);
    let function_index = imported_function_count + *next_defined_function;
    *next_defined_function += 1;

    let realloc_name = resolve.wasm_export_name(mangling, WasmExport::Realloc);
    exports.export(&realloc_name, ExportKind::Func, function_index);

    let mut body = WasmFunction::new([(3, ValType::I32)]);
    body.instruction(&Instruction::GlobalGet(0));
    body.instruction(&Instruction::LocalSet(4));
    body.instruction(&Instruction::GlobalGet(0));
    body.instruction(&Instruction::LocalGet(3));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::LocalSet(5));
    body.instruction(&Instruction::LocalGet(5));
    body.instruction(&Instruction::MemorySize(0));
    body.instruction(&Instruction::I32Const(WASM_PAGE_SIZE));
    body.instruction(&Instruction::I32Mul);
    body.instruction(&Instruction::I32GtU);
    body.instruction(&Instruction::If(BlockType::Empty));
    body.instruction(&Instruction::LocalGet(5));
    body.instruction(&Instruction::MemorySize(0));
    body.instruction(&Instruction::I32Const(WASM_PAGE_SIZE));
    body.instruction(&Instruction::I32Mul);
    body.instruction(&Instruction::I32Sub);
    body.instruction(&Instruction::I32Const(WASM_PAGE_SIZE - 1));
    body.instruction(&Instruction::I32Add);
    body.instruction(&Instruction::I32Const(WASM_PAGE_SIZE));
    body.instruction(&Instruction::I32DivU);
    body.instruction(&Instruction::MemoryGrow(0));
    body.instruction(&Instruction::Drop);
    body.instruction(&Instruction::End);
    body.instruction(&Instruction::LocalGet(5));
    body.instruction(&Instruction::GlobalSet(0));
    body.instruction(&Instruction::LocalGet(4));
    body.instruction(&Instruction::End);
    code.function(&body);
}

#[allow(clippy::too_many_arguments)]
fn export_initialize(
    resolve: &Resolve,
    mangling: ManglingAndAbi,
    types: &mut TypeSection,
    type_count: &mut u32,
    functions: &mut FunctionSection,
    exports: &mut ExportSection,
    code: &mut CodeSection,
    imported_function_count: u32,
    next_defined_function: &mut u32,
) {
    let type_index = push_core_type(types, type_count, &[], &[]);
    functions.function(type_index);
    let function_index = imported_function_count + *next_defined_function;
    *next_defined_function += 1;

    let initialize_name = resolve.wasm_export_name(mangling, WasmExport::Initialize);
    exports.export(&initialize_name, ExportKind::Func, function_index);

    let mut body = WasmFunction::new([]);
    body.instruction(&Instruction::End);
    code.function(&body);
}

fn direct_run_function(
    indices: &DirectCoreFunctionIndices,
    config: &DirectCoreConfig,
) -> WasmFunction {
    const DATA_PTR_LOCAL: u32 = 0;
    const DATA_LEN_LOCAL: u32 = 1;
    const SOURCE_PTR_LOCAL: u32 = 2;
    const SOURCE_LEN_LOCAL: u32 = 3;
    const OUTPUT_PTR_LOCAL: u32 = 4;
    const OUTPUT_LEN_LOCAL: u32 = 5;
    const STEPS_PTR_LOCAL: u32 = 6;
    const STEPS_LEN_LOCAL: u32 = 7;
    const ROUTE_PTR_LOCAL: u32 = 8;
    const ROUTE_LEN_LOCAL: u32 = 9;

    let mut body = WasmFunction::new([
        (16, ValType::I32),
        (2, ValType::I64),
        (10, ValType::I32),
        (1, ValType::I64),
        (17, ValType::I32),
        (6, ValType::I32),
        (2, ValType::I64),
        (10, ValType::I32),
        (9, ValType::I32),
        (2, ValType::I64),
        (5, ValType::I32),
        (2, ValType::I64),
        (2, ValType::I32),
        (20, ValType::I32),
        // Trailing i32 scratch group. The last index (DIRECT_CONDITION_RESULT_LOCAL
        // = 107) stashes a Conditional's evaluated bool across its debug-end event.
        (4, ValType::I32),
    ]);

    push_segment_args(&mut body, &config.static_data.manifest);
    push_retptr_arg(&mut body);
    body.instruction(&Instruction::Call(indices.stdlib_init_manifest));
    emit_fail_if_retptr_error(&mut body, indices, SOURCE_PTR_LOCAL, SOURCE_LEN_LOCAL);

    push_retptr_arg(&mut body);
    body.instruction(&Instruction::Call(indices.runtime_load_input));
    emit_fail_if_retptr_error(&mut body, indices, SOURCE_PTR_LOCAL, SOURCE_LEN_LOCAL);
    load_retptr_list(&mut body, DATA_PTR_LOCAL, DATA_LEN_LOCAL);

    body.instruction(&Instruction::I32Const(config.static_data.steps.offset));
    body.instruction(&Instruction::LocalSet(STEPS_PTR_LOCAL));
    body.instruction(&Instruction::I32Const(config.static_data.steps.len_i32()));
    body.instruction(&Instruction::LocalSet(STEPS_LEN_LOCAL));

    emit_build_source(
        &mut body,
        indices,
        DirectVariables::Segment(&config.static_data.variables),
        DATA_PTR_LOCAL,
        DATA_LEN_LOCAL,
        STEPS_PTR_LOCAL,
        STEPS_LEN_LOCAL,
        SOURCE_PTR_LOCAL,
        SOURCE_LEN_LOCAL,
        None,
    );

    emit_run_plan_mapping(
        &mut body,
        indices,
        &config.static_data,
        config.track_events,
        DirectVariables::Segment(&config.static_data.variables),
        &config.run_plan,
        DATA_PTR_LOCAL,
        DATA_LEN_LOCAL,
        STEPS_PTR_LOCAL,
        STEPS_LEN_LOCAL,
        SOURCE_PTR_LOCAL,
        SOURCE_LEN_LOCAL,
        OUTPUT_PTR_LOCAL,
        OUTPUT_LEN_LOCAL,
        ROUTE_PTR_LOCAL,
        ROUTE_LEN_LOCAL,
        &config.static_data.workflow_log_kind,
        &config.static_data.workflow_error_kind,
        None,
        None,
    );

    body.instruction(&Instruction::LocalGet(OUTPUT_PTR_LOCAL));
    body.instruction(&Instruction::LocalGet(OUTPUT_LEN_LOCAL));
    push_retptr_arg(&mut body);
    body.instruction(&Instruction::Call(indices.runtime_complete));
    load_retptr_tag(&mut body);
    body.instruction(&Instruction::End);
    body
}
