//! Execute emitted wait error handling with canonical ABI runtime responses.
//! These imports return Err in linear memory, unlike the native close import
//! which traps itself. This covers the composed SDK runtime boundary.
use super::*;
use crate::direct_wasm::component::WorkflowAbi;
use wasmtime::{ExternType, Linker, Val};

#[derive(Default)]
struct Calls {
    closed: usize,
    failed: usize,
    after_close: Vec<String>,
}

const ORIGINAL_ERROR: &[u8] = b"wait interval failed";

#[test]
fn managed_wait_close_error_cannot_return_a_recoverable_guest_error() {
    for abi in [
        WorkflowAbi::CliRunHttp,
        WorkflowAbi::InvokeHostImports,
        WorkflowAbi::AgentCapabilities,
    ] {
        let graph = serde_json::from_value(serde_json::json!({
            "durable": true, "entryPoint": "wait", "steps": {
                "wait": {"id": "wait", "stepType": "WaitForSignal"},
                "finish": {"id": "finish", "stepType": "Finish"}
            }, "executionPlan": [{"fromStep": "wait", "toStep": "finish"}]
        }))
        .unwrap();
        let manifest = super::super::manifest::build_direct_workflow_manifest(&graph).unwrap();
        let config =
            DirectCoreConfig::new(&manifest, &manifest.to_canonical_json().unwrap(), false)
                .unwrap()
                .with_abi(abi);
        let (resolve, world) = build_direct_component_resolve_configured(
            &[],
            abi,
            false,
            Some("wait-test"),
            &Default::default(),
            false,
        )
        .unwrap();
        let bytes = emit_direct_core_module(&resolve, world, &config).unwrap();
        let engine = wasmtime::Engine::default();
        let module = wasmtime::Module::new(&engine, bytes).unwrap();
        let memory_name = module
            .exports()
            .find(|export| matches!(export.ty(), ExternType::Memory(_)))
            .unwrap()
            .name()
            .to_owned();
        let entry_name = module
            .exports()
            .find(|export| {
                export
                    .name()
                    .ends_with(if matches!(abi, WorkflowAbi::CliRunHttp) {
                        "|run"
                    } else {
                        "|invoke"
                    })
            })
            .unwrap()
            .name()
            .to_owned();

        for close_fails in [false, true] {
            let mut linker = Linker::<Calls>::new(&engine);
            for import in module.imports() {
                let ExternType::Func(ty) = import.ty() else {
                    panic!("non-function import")
                };
                let name = import.name().to_owned();
                let memory_name = memory_name.clone();
                linker
                    .func_new(
                        import.module(),
                        import.name(),
                        ty,
                        move |mut caller, args, results| {
                            if caller.data().closed != 0 {
                                caller.data_mut().after_close.push(name.clone());
                            }
                            let memory = caller
                                .get_export(&memory_name)
                                .unwrap()
                                .into_memory()
                                .unwrap();
                            let retptr = args.last().and_then(Val::i32).unwrap_or(0) as usize;
                            assert!(results.is_empty(), "unexpected direct result from {name}");
                            let mut response = [0u8; 48];
                            match name.as_str() {
                                "init-manifest" | "load-input" | "build-source" | "instance-id"
                                | "wait-signal-id" | "wait-timeout-ms" | "wait-event"
                                | "register-input" | "custom-event" => {}
                                "wait-poll-interval-ms-scoped" => {
                                    let address = memory.data_size(&caller) - 128;
                                    memory.write(&mut caller, address, ORIGINAL_ERROR)?;
                                    response[0] = 1;
                                    response[4..8].copy_from_slice(&(address as u32).to_le_bytes());
                                    response[8..12].copy_from_slice(
                                        &(ORIGINAL_ERROR.len() as u32).to_le_bytes(),
                                    );
                                }
                                "close-input" => {
                                    caller.data_mut().closed += 1;
                                    response[0] = u8::from(close_fails);
                                    // A successful closure returns InputState::Closed.
                                    response[4] = 2;
                                }
                                "fail" | "invoke-error-fields" => {
                                    let address = args[0].i32().unwrap() as usize;
                                    let len = args[1].i32().unwrap() as usize;
                                    assert_eq!(
                                        &memory.data(&caller)[address..address + len],
                                        ORIGINAL_ERROR,
                                        "successful closure must preserve the original failure"
                                    );
                                    if name == "fail" {
                                        caller.data_mut().failed += 1;
                                    }
                                }
                                other => {
                                    return Err(wasmtime::format_err!(
                                        "unexpected wait import {other}"
                                    ));
                                }
                            }
                            memory.write(&mut caller, retptr, &response)?;
                            Ok(())
                        },
                    )
                    .unwrap();
            }
            let mut store = wasmtime::Store::new(&engine, Calls::default());
            let instance = linker.instantiate(&mut store, &module).unwrap();
            let entry = instance.get_func(&mut store, &entry_name).unwrap();
            let args = vec![Val::I32(0); entry.ty(&store).params().len()];
            let mut result = [Val::I32(0)];
            let outcome = entry.call(&mut store, &args, &mut result);
            assert_eq!(store.data().closed, 1, "{abi:?}");
            if close_fails {
                let error = outcome.expect_err("unconfirmed close must abort the Store");
                assert_eq!(
                    error.downcast_ref::<wasmtime::Trap>(),
                    Some(&wasmtime::Trap::UnreachableCodeReached),
                    "{error:#}"
                );
                assert!(
                    store.data().after_close.is_empty(),
                    "guest executed after failed close"
                );
                assert_eq!(store.data().failed, 0);
            } else {
                outcome.unwrap();
                assert_eq!(
                    store.data().failed,
                    usize::from(!matches!(abi, WorkflowAbi::AgentCapabilities))
                );
                if matches!(abi, WorkflowAbi::CliRunHttp) {
                    assert_eq!(result[0].i32(), Some(1));
                }
            }
        }
    }
}
