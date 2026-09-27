//! Frozen host ABI: guests compiled against a released WIT version keep
//! linking for as long as a parked run built from them can wake.
//!
//! Each fixture under `frozen_abi/` is a guest wit-component produced from
//! the released `@0.1.0` WIT (a dummy guest, printed once and committed). It
//! is never regenerated: an in-place edit of a released interface changes
//! the types the host binds under the same name, and the fixture stops
//! linking. A change ships as a new package version, linked beside this one.
use super::*;

const CONTROL_API: &str = include_str!("frozen_abi/control-api.wat");
const CONTROL_EXECUTOR: &str = include_str!("frozen_abi/control-executor.wat");
const OPERATION_SCOPE: &str = include_str!("frozen_abi/operation-scope.wat");
const SUSPENSION_CONTEXT: &str = include_str!("frozen_abi/suspension-context.wat");

fn engine() -> Arc<Engine> {
    crate::build_engine(&crate::EngineConfig {
        cache_dir: None,
        ..Default::default()
    })
    .expect("engine")
}

fn fixture(engine: &Engine, name: &str, wat: &str) -> Component {
    Component::new(engine, wat).unwrap_or_else(|error| panic!("{name} fixture: {error:#}"))
}

/// The released import names the fixtures pin; the constants may move on to
/// a new version, these may not.
#[test]
fn fixtures_import_the_released_names() {
    let engine = engine();
    for (wat, name) in [
        (CONTROL_API, "runtara:control/api@0.1.0"),
        (CONTROL_EXECUTOR, "runtara:control/executor@0.1.0"),
        (OPERATION_SCOPE, "runtara:workflow-operation/scope@0.1.0"),
        (SUSPENSION_CONTEXT, "runtara:agent-suspension/context@0.1.0"),
    ] {
        let component = fixture(&engine, name, wat);
        assert!(
            component
                .component_type()
                .imports(&engine)
                .any(|(import, _)| import == name),
            "{name} fixture imports {name}"
        );
    }
}

/// Workflow roots: the operation scope, the suspension context, the control
/// executor forwarder and the `denied` control API all bind 0.1.0.
#[test]
fn workflow_stores_link_every_frozen_0_1_0_guest() {
    let engine = engine();
    let executor = WorkflowExecutor::new(engine.clone()).expect("workflow executor");
    for (name, wat) in [
        ("control/api", CONTROL_API),
        ("control/executor", CONTROL_EXECUTOR),
        ("workflow-operation/scope", OPERATION_SCOPE),
        ("agent-suspension/context", SUSPENSION_CONTEXT),
    ] {
        executor
            .linker
            .instantiate_pre(&fixture(&engine, name, wat))
            .unwrap_or_else(|error| panic!("workflow linker refused frozen {name}: {error:#}"));
    }
}

/// Agent stores (dispatcher, trusted, isolated capabilities): the context
/// and the `denied` control stubs bind 0.1.0.
#[test]
fn agent_stores_link_the_frozen_0_1_0_guests() {
    let engine = engine();
    let linker = crate::registry::build_linker(&engine).expect("agent linker");
    for (name, wat) in [
        ("control/api", CONTROL_API),
        ("control/executor", CONTROL_EXECUTOR),
        ("agent-suspension/context", SUSPENSION_CONTEXT),
    ] {
        linker
            .instantiate_pre(&fixture(&engine, name, wat))
            .unwrap_or_else(|error| panic!("agent linker refused frozen {name}: {error:#}"));
    }
}

/// The control executor's fresh stores bind the real 0.1.0 API.
#[test]
fn control_executor_stores_link_the_frozen_0_1_0_api() {
    let engine = engine();
    let linker = crate::control_executor::control_linker(&engine).expect("control linker");
    for (name, wat) in [
        ("control/api", CONTROL_API),
        ("control/executor", CONTROL_EXECUTOR),
        ("agent-suspension/context", SUSPENSION_CONTEXT),
    ] {
        linker
            .instantiate_pre(&fixture(&engine, name, wat))
            .unwrap_or_else(|error| panic!("control linker refused frozen {name}: {error:#}"));
    }
}

/// The check has teeth: a guest whose 0.1.0 shape differs from the host's
/// binding is refused at link time, so an in-place edit of released WIT on
/// the host side would fail the fixtures above the same way.
#[test]
fn a_drifted_0_1_0_shape_does_not_link() {
    let engine = engine();
    let drifted = Component::new(
        &engine,
        r#"(component (import "runtara:control/executor@0.1.0" (instance
            (export "invoke" (func async (param "capability-id" string) (param "extra" u32))))))"#,
    )
    .expect("drifted guest compiles");
    let executor = WorkflowExecutor::new(engine.clone()).expect("workflow executor");
    assert!(executor.linker.instantiate_pre(&drifted).is_err());
}
