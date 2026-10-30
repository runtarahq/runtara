//! Artifacts built for the host ABI the workflow API replaced.
//!
//! Workflows and agents reach the platform only over the workflow API now.
//! A component built before that still imports the old host interfaces,
//! which nothing links: refuse it up front with what to do, instead of
//! wasmtime's report of a missing implementation.
use wasmtime::Engine;
use wasmtime::component::Component;

/// Interface-name prefixes of the removed host ABI. `runtara:trusted-artifacts/`
/// pins are not imports of the old ABI.
const REMOVED: &[&str] = &[
    "runtara:workflow/",
    "runtara:host/",
    "runtara:control/",
    "runtara:trusted/executor@",
    "runtara:agent/continuation@",
];

/// The first removed interface `name` names, if any.
fn removed(name: &str) -> bool {
    REMOVED.iter().any(|prefix| name.starts_with(prefix))
}

/// Refuse `component` when it imports the removed host ABI.
pub(crate) fn refuse(engine: &Engine, component: &Component) -> anyhow::Result<()> {
    if let Some((name, _)) = component
        .component_type()
        .imports(engine)
        .find(|(name, _)| removed(name))
    {
        anyhow::bail!(
            "this component was built for the removed host ABI (it imports `{name}`): \
             recompile the workflow, or rebuild the agent, for the workflow API"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_of_the_removed_host_abi_are_refused_with_what_to_do() {
        let engine = crate::build_engine(&crate::EngineConfig {
            cache_dir: None,
            enable_epoch_interruption: false,
        })
        .unwrap();
        let importing = |name: &str| {
            Component::new(
                &engine,
                format!(r#"(component (import "{name}" (instance (export "f" (func)))))"#),
            )
            .unwrap()
        };
        for name in [
            "runtara:workflow/runtime@1.0.0",
            "runtara:workflow/tasks@1.0.0",
            "runtara:host/timers@1.0.0",
            "runtara:control/api@1.0.0",
            "runtara:trusted/executor@1.0.0",
            "runtara:agent/continuation@1.0.0",
        ] {
            let error = refuse(&engine, &importing(name)).unwrap_err().to_string();
            assert!(
                error.contains(name) && error.contains("recompile"),
                "{error}"
            );
        }
        for name in [
            "runtara:api/tasks@1.0.0",
            "runtara:api/run@1.0.0",
            "wasi:http/client@0.3.0",
        ] {
            assert!(refuse(&engine, &importing(name)).is_ok(), "{name}");
        }
    }
}
