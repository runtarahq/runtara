//! Exercise the actual package wire format through Wasmtime validation/execution.
use runtara_component_host::{EngineConfig, build_engine};
use runtara_workflow_wit::isolation_package::{
    Binding, PackageLimits, append, artifact_digest, parse,
};
use wasmtime::{
    Store,
    component::{Component, Linker},
};

#[tokio::test]
async fn packaged_root_and_deduplicated_child_execute_as_real_components() {
    let engine = build_engine(&EngineConfig {
        cache_dir: None,
        ..Default::default()
    })
    .unwrap();
    let root = wat::parse_str(
        r#"(component
        (core module $m (func (export "run") (result i32) i32.const 7))
        (core instance $i (instantiate $m))
        (func (export "run") (result u32) (canon lift (core func $i "run"))))"#,
    )
    .unwrap();
    let child = wat::parse_str(
        r#"(component
        (core module $m (func (export "run") (param i32) (result i32) local.get 0))
        (core instance $i (instantiate $m))
        (func (export "run") (param "value" u32) (result u32) (canon lift (core func $i "run"))))"#,
    )
    .unwrap();
    let bindings = (0..100)
        .map(|i| Binding {
            id: format!("call-{i}"),
            artifact: artifact_digest(&child),
            interface: "fixture".into(),
        })
        .collect();
    let limits = PackageLimits {
        total_bytes: 65536,
        manifest_bytes: 32768,
        artifacts: 1,
        bindings: 100,
    };
    let package = append(&root, &[&child, &child], bindings, limits).unwrap();
    let parsed = parse(&package, limits).unwrap().unwrap();
    assert_eq!(parsed.artifacts().len(), 1);
    assert_eq!(parsed.root, root);
    // Exercise the exact worker compiler and trusted-response decoder too.
    // Production transports this response over its process-private pipe.
    use runtara_component_host::precompile::{
        PrecompileRequest, PrecompileResponse, deserialize_trusted_precompiled_component,
        deserialize_trusted_precompiled_package, precompile_artifact,
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workflow.wasm");
    std::fs::write(&path, &package).unwrap();
    let request = PrecompileRequest::for_artifact([7; 32], &path).unwrap();
    let response = PrecompileResponse::Success(precompile_artifact(&request).unwrap());
    // SAFETY: response is the unchanged output of our worker compiler above.
    let compiled =
        unsafe { deserialize_trusted_precompiled_package(&engine, &request, &response) }.unwrap();
    assert_eq!(compiled.artifacts.len(), 1);
    assert_eq!(compiled.bindings.len(), 100);
    // A legacy decoder must refuse to silently discard the child catalog.
    assert!(
        unsafe { deserialize_trusted_precompiled_component(&engine, &request, &response) }.is_err()
    );
    let other = PrecompileRequest::for_artifact([8; 32], &path).unwrap();
    assert!(
        unsafe { deserialize_trusted_precompiled_package(&engine, &other, &response) }.is_err()
    );
    let linker = Linker::<()>::new(&engine);
    let mut store = Store::new(&engine, ());
    store.set_epoch_deadline(1 << 40);
    let component = Component::new(&engine, &package).unwrap();
    let instance = linker
        .instantiate_async(&mut store, &component)
        .await
        .unwrap();
    assert_eq!(
        instance
            .get_typed_func::<(), (u32,)>(&mut store, "run")
            .unwrap()
            .call_async(&mut store, ())
            .await
            .unwrap(),
        (7,)
    );
    let (_, bytes) = parsed.resolve("call-99").unwrap();
    let component = Component::new(&engine, bytes).unwrap();
    let mut child_store = Store::new(&engine, ());
    child_store.set_epoch_deadline(1 << 40);
    let instance = linker
        .instantiate_async(&mut child_store, &component)
        .await
        .unwrap();
    assert_eq!(
        instance
            .get_typed_func::<(u32,), (u32,)>(&mut child_store, "run")
            .unwrap()
            .call_async(&mut child_store, (42,))
            .await
            .unwrap(),
        (42,)
    );
    let mut prepared_store = Store::new(&engine, ());
    prepared_store.set_epoch_deadline(1 << 40);
    let instance = linker
        .instantiate_async(&mut prepared_store, &compiled.root)
        .await
        .unwrap();
    assert_eq!(
        instance
            .get_typed_func::<(), (u32,)>(&mut prepared_store, "run")
            .unwrap()
            .call_async(&mut prepared_store, ())
            .await
            .unwrap(),
        (7,)
    );
    let child = compiled.artifacts.get(&artifact_digest(&child)).unwrap();
    let mut prepared_child = Store::new(&engine, ());
    prepared_child.set_epoch_deadline(1 << 40);
    let instance = linker
        .instantiate_async(&mut prepared_child, child)
        .await
        .unwrap();
    assert_eq!(
        instance
            .get_typed_func::<(u32,), (u32,)>(&mut prepared_child, "run")
            .unwrap()
            .call_async(&mut prepared_child, (42,))
            .await
            .unwrap(),
        (42,)
    );
}
