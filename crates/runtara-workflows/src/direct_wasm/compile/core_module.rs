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
    DirectCoreFunctionIndices, DirectCoreImportIndices, agent_id_for_import, import_core_function,
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
    /// Top-level export shape (see `component::WorkflowAbi`). Defaults to the
    /// legacy `wasi:cli/run`; set via [`Self::with_abi`].
    pub(super) abi: crate::direct_wasm::component::WorkflowAbi,
    /// Opt-in gate for the store-freeing durable-sleep lowering (see
    /// [`DirectCoreFunctionIndices::store_freeing_sleep`]). Defaults to false —
    /// the blocking, byte-preserved path — and is set via
    /// [`Self::with_store_freeing_sleep`].
    pub(super) store_freeing_sleep: bool,
    /// When true, the component imports no `runtara:workflow-runtime/runtime`,
    /// so the emitter must NOT lower any `runtime.*` call — the terminal
    /// `complete`/`fail` are dropped and the result travels solely in-band via
    /// the invoke return value. Only valid for a pure workflow under the invoke
    /// export (see [`Self::with_omit_runtime`]).
    pub(super) omit_runtime: bool,
}

impl DirectCoreConfig {
    /// Test constructor pinned to the LEGACY `wasi:cli/run` body shape — the
    /// structural lowering tests describe that sequence (load-input included).
    /// Invoke-shape structure is asserted by explicit `.with_abi` tests and
    /// the execution battery.
    #[cfg(test)]
    pub(super) fn new(
        manifest: &DirectWorkflowManifest,
        manifest_json: &[u8],
        track_events: bool,
    ) -> Result<Self, DirectCompileError> {
        Self::new_inner(manifest, manifest_json, track_events, None)
            .map(|config| config.with_abi(crate::direct_wasm::component::WorkflowAbi::CliRunHttp))
    }

    pub(super) fn new_with_workflow_id(
        manifest: &DirectWorkflowManifest,
        manifest_json: &[u8],
        track_events: bool,
        workflow_id: &str,
    ) -> Result<Self, DirectCompileError> {
        Self::new_inner(manifest, manifest_json, track_events, Some(workflow_id))
    }

    /// Override the export shape.
    pub(super) fn with_abi(mut self, abi: crate::direct_wasm::component::WorkflowAbi) -> Self {
        self.abi = abi;
        // Parallel Split windows require an async-TYPED root task (the invoke
        // shapes); the legacy sync-typed `wasi:cli/run` root always compiles
        // sequentially.
        self.static_data.parallel_enabled =
            !matches!(abi, crate::direct_wasm::component::WorkflowAbi::CliRunHttp);
        self
    }

    /// Enable the store-freeing durable-sleep lowering (opt-in; default off).
    pub(super) fn with_store_freeing_sleep(mut self, enabled: bool) -> Self {
        self.store_freeing_sleep = enabled;
        self
    }

    /// Compile with no runtime import (agent-shaped; opt-in, default off).
    pub(super) fn with_omit_runtime(mut self, enabled: bool) -> Self {
        self.omit_runtime = enabled;
        self
    }

    fn new_inner(
        manifest: &DirectWorkflowManifest,
        manifest_json: &[u8],
        track_events: bool,
        workflow_id: Option<&str>,
    ) -> Result<Self, DirectCompileError> {
        let variables_json = direct_core_variables_json(&manifest.graph.variables, workflow_id)?;
        Ok(Self {
            abi: crate::direct_wasm::component::WorkflowAbi::default(),
            store_freeing_sleep: false,
            omit_runtime: false,
            run_plan: direct_run_plan(manifest)?,
            static_data: DirectCoreStaticData::new_with_child_workflows(
                &manifest.graph,
                &manifest.child_workflows,
                manifest_json,
                &variables_json,
                DIRECT_EMPTY_STEPS_CONTEXT,
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

    // Parallel-Split extra CORE imports (docs/wasip3-parallelism.md Phase 3):
    // the CM-async waitable builtins from the legacy `$root` module, plus an
    // `[async-lower]invoke` per agent referenced by an eligible parallel
    // window. Emitted only when such a window exists, so sequential-only
    // workflows keep a byte-identical import section. wit-component's legacy
    // name mangling turns these into `canon lower ... async` / the waitable
    // canon builtins at encode time.
    let parallel_pools =
        super::split_parallel::parallel_agent_pools(&config.static_data, &config.run_plan);
    if !parallel_pools.is_empty() {
        let builtin = |field: &str,
                       params: &[ValType],
                       results: &[ValType],
                       types: &mut TypeSection,
                       type_count: &mut u32,
                       imports: &mut ImportSection,
                       count: &mut u32|
         -> u32 {
            let type_index = {
                let index = *type_count;
                types
                    .ty()
                    .function(params.iter().copied(), results.iter().copied());
                *type_count += 1;
                index
            };
            imports.import(
                "$root",
                field,
                wasm_encoder::EntityType::Function(type_index),
            );
            let function_index = *count;
            *count += 1;
            function_index
        };
        import_indices.waitable_set_new = Some(builtin(
            "[waitable-set-new]",
            &[],
            &[ValType::I32],
            &mut types,
            &mut type_count,
            &mut imports,
            &mut imported_function_count,
        ));
        import_indices.waitable_set_wait = Some(builtin(
            "[waitable-set-wait]",
            &[ValType::I32, ValType::I32],
            &[ValType::I32],
            &mut types,
            &mut type_count,
            &mut imports,
            &mut imported_function_count,
        ));
        import_indices.waitable_set_drop = Some(builtin(
            "[waitable-set-drop]",
            &[ValType::I32],
            &[],
            &mut types,
            &mut type_count,
            &mut imports,
            &mut imported_function_count,
        ));
        import_indices.waitable_join = Some(builtin(
            "[waitable-join]",
            &[ValType::I32, ValType::I32],
            &[],
            &mut types,
            &mut type_count,
            &mut imports,
            &mut imported_function_count,
        ));
        import_indices.subtask_drop = Some(builtin(
            "[subtask-drop]",
            &[ValType::I32],
            &[],
            &mut types,
            &mut type_count,
            &mut imports,
            &mut imported_function_count,
        ));

        let is_pool_member = |agent_id: &str| -> bool {
            if parallel_pools.contains_key(agent_id) {
                return true;
            }
            // "<base>-par<n>" phantom member of a pooled base?
            agent_id.rfind("-par").is_some_and(|split_at| {
                let (base, suffix) = agent_id.split_at(split_at);
                suffix[4..].parse::<u32>().ok().is_some_and(|member| {
                    parallel_pools.get(base).is_some_and(|pool| member < *pool)
                })
            })
        };
        for (name, import) in &world.imports {
            let WorldItem::Interface { id, .. } = import else {
                continue;
            };
            let Some(agent_id) = agent_id_for_import(resolve, Some(name)) else {
                continue;
            };
            if !is_pool_member(&agent_id) {
                continue;
            }
            for function in resolve.interfaces[*id].functions.values() {
                if function.name != "invoke" {
                    continue;
                }
                let async_mangling =
                    ManglingAndAbi::Legacy(wit_parser::LiftLowerAbi::AsyncCallback);
                let signature = resolve.wasm_signature(async_mangling.import_variant(), function);
                let type_index = push_core_type(
                    &mut types,
                    &mut type_count,
                    &signature.params,
                    &signature.results,
                );
                let (module, field) = resolve.wasm_import_name(
                    async_mangling,
                    wit_parser::WasmImport::Func {
                        interface: Some(name),
                        func: function,
                    },
                );
                imports.import(
                    &module,
                    &field,
                    wasm_encoder::EntityType::Function(type_index),
                );
                import_indices.agent_invokes_async.insert(
                    agent_id.clone(),
                    super::DirectAgentInvokeImport {
                        function_index: imported_function_count,
                        params: signature.params.clone(),
                    },
                );
                imported_function_count += 1;
            }
        }
    }

    let import_indices =
        import_indices.require_all(config.abi, config.store_freeing_sleep, config.omit_runtime)?;

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

    let body = if is_wasi_cli_run_export(resolve, interface, function)
        || super::core_imports::is_lifecycle_invoke_export(resolve, interface, function)
        || super::core_imports::is_capabilities_invoke_export(resolve, interface, function)
    {
        // The entry export of the current ABI (the world declares exactly one):
        // `wasi:cli/run` under CliRunHttp, `lifecycle.invoke` under
        // InvokeHostImports, `capabilities.invoke` under AgentCapabilities.
        // `direct_run_function` shapes its prologue, param fold, and return
        // convention from `config.abi` and the export's param count.
        direct_run_function(import_indices, config, signature.params.len())
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

/// Canonical declared-local groups for the run function under ZERO export
/// params (the `wasi:cli/run` shape). Every other ABI derives its declared
/// locals by dropping its export params off the FRONT of this list
/// ([`drop_leading_locals`]): `wasi:cli/run` takes 0 params (uses this list
/// verbatim), `lifecycle.invoke(input)` takes 2 (its `input` folds onto locals
/// 0/1), and `capabilities.invoke(capability-id, input)` takes 4. Because
/// the ~100 hand-assigned `DIRECT_*_LOCAL` indices are ABSOLUTE, dropping params
/// off the front keeps each surviving declared local at its original absolute
/// index with its original type — the invariant the lowerers depend on.
const CANONICAL_LOCAL_GROUPS: &[(u32, ValType)] = &[
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
    // Trailing i32 scratch group. Notable indices: 107
    // (DIRECT_CONDITION_RESULT_LOCAL) stashes a Conditional's evaluated bool
    // across its debug-end event; 108 (DIRECT_SPLIT_HEAP_BASE_LOCAL) holds the
    // Split/While loop's heap watermark; 109 (DIRECT_AI_HEAP_BASE_LOCAL) holds
    // the AiAgent loop's heap watermark; 110-115 (DIRECT_AGENT_ATTEMPT_*) are
    // the durable Agent retry per-attempt-result checkpoint scratch.
    (20, ValType::I32),
    // 116-117 spare; 118-123 are the parallel-Split scratch (DIRECT_PSPLIT_*,
    // see docs/wasip3-parallelism.md Phase 3).
    (20, ValType::I32),
];

/// Drop `n` leading local slots from `groups`, splitting (never merging) the
/// group the drop lands in so every surviving slot keeps its absolute index and
/// type. Used to fold export params onto the front of [`CANONICAL_LOCAL_GROUPS`].
fn drop_leading_locals(groups: &[(u32, ValType)], n: u32) -> Vec<(u32, ValType)> {
    let mut remaining = n;
    let mut out = Vec::new();
    for &(count, ty) in groups {
        if remaining == 0 {
            out.push((count, ty));
        } else if remaining >= count {
            remaining -= count;
        } else {
            out.push((count - remaining, ty));
            remaining = 0;
        }
    }
    out
}

fn direct_run_function(
    indices: &DirectCoreFunctionIndices,
    config: &DirectCoreConfig,
    export_param_count: usize,
) -> WasmFunction {
    use crate::direct_wasm::component::WorkflowAbi;

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

    // Fold the export params onto the front of the canonical local layout. The
    // input list's two i32s land on DATA_PTR/DATA_LEN (0/1) under
    // `lifecycle.invoke` (2 params); under `capabilities.invoke(capability-id,
    // input)` (4 params: cap-id + input, ≤ the 16-param flat limit so passed
    // DIRECTLY) the input lands on params 2/3 and is copied to 0/1 in the
    // prologue below. Every surviving declared local keeps its absolute
    // DIRECT_*_LOCAL index.
    let mut body = WasmFunction::new(drop_leading_locals(
        CANONICAL_LOCAL_GROUPS,
        export_param_count as u32,
    ));

    // `capabilities.invoke` flattens to (cap-ptr @0, cap-len @1, input-ptr @2,
    // input-len @3). The input params ALIAS the SOURCE locals (2/3), which
    // init-manifest's error path scribbles into — so stash the input onto
    // DATA_PTR/DATA_LEN (0/1) BEFORE init-manifest runs. (The cap-id at 0/1 is
    // unused: a workflow-agent has a single self-capability.)
    if matches!(config.abi, WorkflowAbi::AgentCapabilities) {
        body.instruction(&Instruction::LocalGet(2));
        body.instruction(&Instruction::LocalSet(DATA_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(3));
        body.instruction(&Instruction::LocalSet(DATA_LEN_LOCAL));
    }

    push_segment_args(&mut body, &config.static_data.manifest);
    push_retptr_arg(&mut body);
    body.instruction(&Instruction::Call(indices.stdlib_init_manifest));
    emit_fail_if_retptr_error(&mut body, indices, SOURCE_PTR_LOCAL, SOURCE_LEN_LOCAL);

    match config.abi {
        WorkflowAbi::CliRunHttp => {
            push_retptr_arg(&mut body);
            body.instruction(&Instruction::Call(indices.runtime_load_input));
            emit_fail_if_retptr_error(&mut body, indices, SOURCE_PTR_LOCAL, SOURCE_LEN_LOCAL);
            load_retptr_list(&mut body, DATA_PTR_LOCAL, DATA_LEN_LOCAL);
        }
        WorkflowAbi::InvokeHostImports => {
            // The input envelope arrived as the call argument — params 0/1 ARE
            // (DATA_PTR, DATA_LEN); no load-input round-trip.
        }
        WorkflowAbi::AgentCapabilities => {
            // Input already stashed onto DATA_PTR/DATA_LEN above (before
            // init-manifest could clobber the aliased SOURCE params).
        }
    }

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

    // The additive `runtime.complete` records terminal status/output host-side
    // during the migration. Suppressed when the runtime is omitted (nothing to
    // call) and under AgentCapabilities even with the runtime imported (a
    // durable workflow-agent composed into a parent shares the PARENT
    // instance's runtime — completing it here would finish the parent
    // mid-flight); the invoke return value is the sole terminal result.
    if !config.omit_runtime && !matches!(config.abi, WorkflowAbi::AgentCapabilities) {
        body.instruction(&Instruction::LocalGet(OUTPUT_PTR_LOCAL));
        body.instruction(&Instruction::LocalGet(OUTPUT_LEN_LOCAL));
        push_retptr_arg(&mut body);
        body.instruction(&Instruction::Call(indices.runtime_complete));
    }
    match config.abi {
        WorkflowAbi::CliRunHttp => {
            load_retptr_tag(&mut body);
        }
        WorkflowAbi::InvokeHostImports => {
            // The terminal result travels as the return value:
            // Ok(outcome::completed(output)).
            emit_invoke_ok_completed_return(&mut body, OUTPUT_PTR_LOCAL, OUTPUT_LEN_LOCAL);
        }
        WorkflowAbi::AgentCapabilities => {
            // Agent capability shape: Ok(output) as a bare list<u8>.
            emit_capabilities_ok_return(&mut body, OUTPUT_PTR_LOCAL, OUTPUT_LEN_LOCAL);
        }
    }
    body.instruction(&Instruction::End);
    body
}

/// Write `Ok(outcome::completed(output))` for the invoke export into the
/// fixed result area and leave its pointer on the stack.
///
/// Canonical-ABI layout of `result<outcome, error-info>` (payload align 8):
/// result disc u8 @0; ok arm = outcome @8: disc u8 @8 (0 = completed),
/// payload list<u8> @12: ptr @12, len @16. The area is the low retptr
/// scratch at offset 0 — dead by construction here (no host call ever runs
/// between this write and the canonical lift; post-return is a no-op) and
/// 8-aligned as the ABI requires.
pub(super) fn emit_invoke_ok_completed_return(
    body: &mut WasmFunction,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    // Zero the header region so both discriminants read 0 (ok/completed).
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Const(24));
    body.instruction(&Instruction::MemoryFill(0));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
        offset: 12,
        align: 2,
        memory_index: 0,
    }));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalGet(output_len_local));
    body.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
        offset: 16,
        align: 2,
        memory_index: 0,
    }));
    // The return value: the result area's address.
    body.instruction(&Instruction::I32Const(0));
}

/// Write `Ok(output)` for the agent-capabilities export into the fixed result
/// area and leave its pointer on the stack.
///
/// Canonical-ABI layout of `result<list<u8>, error-info>` (payload align 8 —
/// error-info carries a `u64`): result disc u8 @0 (0 = ok); ok payload = the
/// `list<u8>` directly at @8: ptr @8, len @12. (Contrast the lifecycle export's
/// `result<outcome, error-info>`, whose ok arm is `outcome::completed` — a
/// variant disc @8 plus the list at @12/@16.) The error arm is `error-info` at
/// @8, byte-identical to the lifecycle error arm, so the shared err writer
/// applies unchanged.
pub(super) fn emit_capabilities_ok_return(
    body: &mut WasmFunction,
    output_ptr_local: u32,
    output_len_local: u32,
) {
    // Zero the header (result disc @0 = 0 = ok) and the ok payload slot.
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::I32Const(16));
    body.instruction(&Instruction::MemoryFill(0));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalGet(output_ptr_local));
    body.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
        offset: 8,
        align: 2,
        memory_index: 0,
    }));
    body.instruction(&Instruction::I32Const(0));
    body.instruction(&Instruction::LocalGet(output_len_local));
    body.instruction(&Instruction::I32Store(wasm_encoder::MemArg {
        offset: 12,
        align: 2,
        memory_index: 0,
    }));
    body.instruction(&Instruction::I32Const(0));
}
