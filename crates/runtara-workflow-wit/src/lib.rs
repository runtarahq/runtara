// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Canonical WIT contracts for direct-emitted workflow components.

/// First workflow WIT ABI version.
pub const WORKFLOW_WIT_VERSION: &str = "0.1.0";

/// WIT package name for the reusable JSON stdlib component.
pub const STDLIB_PACKAGE: &str = "runtara:workflow-stdlib@0.1.0";

/// WIT package name for the runtime/SDK lifecycle component.
pub const RUNTIME_PACKAGE: &str = "runtara:workflow-runtime@0.1.0";

/// WIT package name for safe runtime connection resolution.
pub const CONNECTION_RESOLVER_PACKAGE: &str = "runtara:connection-resolver@0.1.0";

/// Fully-qualified component import name of the connection resolver.
pub const CONNECTION_RESOLVER_INTERFACE_NAME: &str = "runtara:connection-resolver/resolver@0.1.0";

/// WIT package name for the neutral shared ABI vocabulary.
pub const ABI_PACKAGE: &str = "runtara:abi@0.1.0";

/// WIT package name for the workflow invoke-export contract.
pub const LIFECYCLE_PACKAGE: &str = "runtara:workflow-lifecycle@0.2.0";

/// Fully-qualified component export name of the lifecycle interface — what a
/// workflow compiled with the invoke ABI exports instead of `wasi:cli/run`.
pub const LIFECYCLE_INTERFACE_NAME: &str = "runtara:workflow-lifecycle/lifecycle@0.2.0";

/// The 0.1.0 (sync-typed invoke) interface name — exported by artifacts
/// compiled before ABI v2. The executor accepts both; new compiles always
/// export 0.2.0.
pub const LIFECYCLE_INTERFACE_NAME_V1: &str = "runtara:workflow-lifecycle/lifecycle@0.1.0";

/// WIT text for `runtara:workflow-stdlib@0.1.0`.
pub const STDLIB_WIT: &str = include_str!("../wit/stdlib/runtara-workflow-stdlib.wit");

/// WIT text for `runtara:workflow-runtime@0.1.0`.
pub const RUNTIME_WIT: &str = include_str!("../wit/runtime/runtara-workflow-runtime.wit");

/// WIT text for `runtara:connection-resolver@0.1.0`.
pub const CONNECTION_RESOLVER_WIT: &str =
    include_str!("../wit/connection-resolver/runtara-connection-resolver.wit");

/// WIT text for `runtara:abi@0.1.0` (the neutral shared vocabulary).
pub const ABI_WIT: &str = include_str!("../wit/lifecycle/deps/abi/runtara-abi.wit");

/// WIT text for `runtara:workflow-lifecycle@0.2.0`.
pub const LIFECYCLE_WIT: &str = include_str!("../wit/lifecycle/runtara-workflow-lifecycle.wit");

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{CONNECTION_RESOLVER_PACKAGE, RUNTIME_PACKAGE, STDLIB_PACKAGE};
    use wit_parser::{Resolve, WorldItem};

    fn crate_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn stdlib_wit_parses_and_exports_json_world() {
        let mut resolve = Resolve::default();
        let package_id = resolve
            .push_file(crate_dir().join("wit/stdlib/runtara-workflow-stdlib.wit"))
            .expect("stdlib WIT parses");
        let package = &resolve.packages[package_id];

        assert_eq!(package.name.to_string(), STDLIB_PACKAGE);
        let interface_id = package.interfaces["json"];
        let interface = &resolve.interfaces[interface_id];
        for function in [
            "init-manifest",
            "build-source",
            "apply-mapping",
            "eval-condition",
            "process-switch",
            "value-switch",
            "split-items",
            "split-item-count",
            "split-item",
            "split-iteration-variables",
            "split-validate-input",
            "split-validate-output",
            "split-initial-results",
            "split-append-output",
            "split-append-error",
            "split-output",
            "split-cache-key",
            "split-result",
            "split-output-from-result",
            "while-max-iterations",
            "while-initial-state",
            "while-condition-source",
            "while-condition",
            "while-iteration-variables",
            "while-advance-state",
            "while-output",
            "filter",
            "log-event",
            "log",
            "error-event",
            "error",
            "error-steps",
            "group-by",
            "delay-duration-ms",
            "delay",
            "delay-sleep-key",
            "invoke-error-fields",
            "breakpoint-key",
            "breakpoint-event",
            "wait-signal-id",
            "wait-timeout-ms",
            "wait-timeout-error",
            "wait-on-wait-variables",
            "wait-on-wait-error",
            "wait-poll-interval-ms",
            "wait-event",
            "wait-debug-start",
            "wait-output",
            "retry-sleep-key",
            "retry-delay-ms",
            "workflow-error-retryable",
            "workflow-error-rate-limited",
            "workflow-error-retry-after-ms",
            "agent-output",
            "agent-validate-input",
            "agent-connection-id",
            "agent-connection-input",
            "agent-cache-key",
            "agent-retry-sleep-key",
            "agent-attempt-result-key",
            "agent-attempt-envelope",
            "agent-retry-delay-ms",
            "agent-error-info",
            "agent-retry-error-info",
            "agent-error",
            "agent-error-from-info",
            "agent-debug-error",
            "step-debug-start",
            "step-debug-end",
        ] {
            assert!(
                interface.functions.contains_key(function),
                "missing stdlib function {function}"
            );
        }

        let world_id = package.worlds["workflow-stdlib"];
        let world = &resolve.worlds[world_id];
        assert!(world.imports.is_empty());
        assert_eq!(world.exports.len(), 1);
        assert!(
            world
                .exports
                .values()
                .any(|item| matches!(item, WorldItem::Interface { id, .. } if *id == interface_id))
        );
    }

    #[test]
    fn connection_resolver_wit_parses_and_exports_universal_operations() {
        let mut resolve = Resolve::default();
        let package_id = resolve
            .push_file(crate_dir().join("wit/connection-resolver/runtara-connection-resolver.wit"))
            .expect("connection resolver WIT parses");
        let package = &resolve.packages[package_id];

        assert_eq!(package.name.to_string(), CONNECTION_RESOLVER_PACKAGE);
        let interface_id = package.interfaces["resolver"];
        let interface = &resolve.interfaces[interface_id];
        for function in ["describe", "resolve-resource"] {
            assert!(
                interface.functions.contains_key(function),
                "missing connection resolver function {function}"
            );
        }

        let world_id = package.worlds["connection-resolver"];
        let world = &resolve.worlds[world_id];
        assert!(world.imports.is_empty());
        assert_eq!(world.exports.len(), 1);
    }

    #[test]
    fn abi_wit_parses_and_defines_shared_types() {
        let mut resolve = Resolve::default();
        let package_id = resolve
            .push_file(crate_dir().join("wit/lifecycle/deps/abi/runtara-abi.wit"))
            .expect("abi WIT parses");
        let package = &resolve.packages[package_id];
        assert_eq!(package.name.to_string(), super::ABI_PACKAGE);
        let interface = &resolve.interfaces[package.interfaces["types"]];
        for type_name in ["error-info", "connection-info"] {
            assert!(
                interface.types.contains_key(type_name),
                "missing abi type {type_name}"
            );
        }
    }

    #[test]
    fn lifecycle_wit_parses_and_exports_invoke() {
        // The lifecycle package `use`s runtara:abi, so both must be in the
        // resolve — the same way the compiler stages them together via
        // `push_str`. (`push_file` treats each file as a self-contained
        // package and won't resolve cross-package `use`; `push_str` into one
        // resolve does, matching `build_direct_component_resolve_configured`.)
        let mut resolve = Resolve::default();
        resolve
            .push_str("runtara-abi.wit", super::ABI_WIT)
            .expect("abi WIT parses");
        let package_id = resolve
            .push_str("runtara-workflow-lifecycle.wit", super::LIFECYCLE_WIT)
            .expect("lifecycle WIT parses");
        let package = &resolve.packages[package_id];

        assert_eq!(package.name.to_string(), super::LIFECYCLE_PACKAGE);
        let interface_id = package.interfaces["lifecycle"];
        let interface = &resolve.interfaces[interface_id];
        assert!(interface.functions.contains_key("invoke"));
        // error-info is now a `use`d type from runtara:abi; the locally
        // declared types are the wait/wake/outcome set.
        for type_name in ["signal-wait", "wake", "outcome"] {
            assert!(
                interface.types.contains_key(type_name),
                "missing lifecycle type {type_name}"
            );
        }

        let world_id = package.worlds["workflow-lifecycle"];
        let world = &resolve.worlds[world_id];
        // The world imports only the `use`d runtara:abi type(s); its single
        // real export is the lifecycle interface.
        assert!(
            world
                .imports
                .values()
                .all(|item| matches!(item, WorldItem::Type { .. } | WorldItem::Interface { .. })),
            "unexpected non-type import on the lifecycle world"
        );
        assert_eq!(world.exports.len(), 1);
        assert!(
            world
                .exports
                .values()
                .any(|item| matches!(item, WorldItem::Interface { id, .. } if *id == interface_id))
        );
    }

    #[test]
    fn runtime_wit_parses_and_exports_runtime_world() {
        let mut resolve = Resolve::default();
        let package_id = resolve
            .push_file(crate_dir().join("wit/runtime/runtara-workflow-runtime.wit"))
            .expect("runtime WIT parses");
        let package = &resolve.packages[package_id];

        assert_eq!(package.name.to_string(), RUNTIME_PACKAGE);
        let interface_id = package.interfaces["runtime"];
        let interface = &resolve.interfaces[interface_id];
        for function in [
            "load-input",
            "instance-id",
            "complete",
            "fail",
            "custom-event",
            "debug-mode-enabled",
            "breakpoint-pause",
            "heartbeat",
            "is-cancelled",
            "check-signals",
            "poll-custom-signal",
            "now-ms",
            "durable-sleep",
            "blocking-sleep",
            "get-checkpoint",
            "checkpoint",
            "handle-checkpoint-signal",
            "record-retry-attempt",
            "durable-sleep-checkpoint",
        ] {
            assert!(
                interface.functions.contains_key(function),
                "missing runtime function {function}"
            );
        }
        for type_name in ["signal-info", "custom-signal-info", "checkpoint-result"] {
            assert!(
                interface.types.contains_key(type_name),
                "missing runtime type {type_name}"
            );
        }

        let world_id = package.worlds["workflow-runtime"];
        let world = &resolve.worlds[world_id];
        assert!(world.imports.is_empty());
        assert_eq!(world.exports.len(), 1);
        assert!(
            world
                .exports
                .values()
                .any(|item| matches!(item, WorldItem::Interface { id, .. } if *id == interface_id))
        );
    }
}
