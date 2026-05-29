// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Canonical WIT contracts for direct-emitted workflow components.

/// First workflow WIT ABI version.
pub const WORKFLOW_WIT_VERSION: &str = "0.1.0";

/// WIT package name for the reusable JSON stdlib component.
pub const STDLIB_PACKAGE: &str = "runtara:workflow-stdlib@0.1.0";

/// WIT package name for the runtime/SDK lifecycle component.
pub const RUNTIME_PACKAGE: &str = "runtara:workflow-runtime@0.1.0";

/// WIT text for `runtara:workflow-stdlib@0.1.0`.
pub const STDLIB_WIT: &str = include_str!("../wit/stdlib/runtara-workflow-stdlib.wit");

/// WIT text for `runtara:workflow-runtime@0.1.0`.
pub const RUNTIME_WIT: &str = include_str!("../wit/runtime/runtara-workflow-runtime.wit");

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{RUNTIME_PACKAGE, STDLIB_PACKAGE};
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
            "wait-signal-id",
            "wait-timeout-ms",
            "wait-timeout-error",
            "wait-on-wait-variables",
            "wait-on-wait-error",
            "wait-poll-interval-ms",
            "wait-event",
            "wait-output",
            "agent-output",
            "agent-validate-input",
            "agent-connection-input",
            "agent-cache-key",
            "agent-retry-sleep-key",
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
