// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Canonical WIT contracts for direct-emitted workflow components.
//!
//! # ABI rule
//!
//! A parked run wakes on the artifact it was built from, possibly after the
//! host, the control agent or a trusted agent was upgraded. So:
//!
//! - **Every shipped host version stays linked.** The host runs lifecycle
//!   0.1.0 and 0.2.0 exports and binds runtime 0.3.0 and 0.4.0,
//!   connection-resolver 0.1.0 and 0.2.0, and `runtara:control`,
//!   `runtara:workflow-operation` and `runtara:agent-suspension` 0.1.0,
//!   beside whatever is current. Built-in (`runtara:builtin-artifacts/…`)
//!   and trusted (`runtara:trusted-artifacts/…`) pins keep linking as empty
//!   instance imports: a pin the host has since revoked or no longer ships
//!   fails at the call, never at load. Only a control pin that was never
//!   approved is refused at load.
//! - **Approved rows are only revoked.** `approved_builtin_artifacts` rows are
//!   never deleted or rewritten, so an artifact pinning an older approved
//!   control version still loads after an upgrade. The same history records
//!   every installed trusted (S3, Azure) version: a run parked on an older,
//!   still approved trusted pin that is woken or resumed calls the installed
//!   bytes; a start under it, or any launch under a revoked or never
//!   approved pin, fails with `TRUSTED_VERSION_REQUIRED` before any
//!   credential lookup.
//! - **Released WIT is never edited, only versioned.** Wasmtime lifts records,
//!   variants and enums exactly, so adding a case or field in place breaks
//!   every artifact built against the old shape. A change is a new package
//!   version, linked beside the old one.
//!
//! `runtara-component-host`'s `workflow/frozen_abi_tests.rs` holds guests
//! compiled against the released 0.1.0 control, operation and suspension
//! WIT; they must keep linking.

#[cfg(feature = "isolation-package")]
pub mod isolation_package;

pub const OUTBOUND_HTTP_INTERFACE_NAME: &str = "runtara:outbound-http/client@0.1.0";
pub const OUTBOUND_HTTP_WIT: &str = include_str!("../wit/outbound-http/runtara-outbound-http.wit");

/// Host timers (`sleep`, `abort-after`) that workflows and ordinary agents
/// import. The component host links this name; the agent allowlist admits it.
pub const HOST_IO_TIMERS_INTERFACE_NAME: &str = "runtara:host-io/timers@0.1.0";

/// Native database operations available to ordinary WASM agents.
pub const DATABASE_INTERFACE_NAME: &str = "runtara:database/sql@0.1.0";
pub const DATABASE_WIT: &str = include_str!("../wit/database/runtara-database.wit");

/// Generic owned child-execution imports, independent of the DSL schema.
pub const EXECUTION_INTERFACE_NAME: &str = "runtara:workflow-execution/tasks@0.1.0";
pub const EXECUTION_WIT: &str = include_str!("../wit/execution/runtara-workflow-execution.wit");

/// First workflow WIT ABI version.
pub const WORKFLOW_WIT_VERSION: &str = "0.1.0";

/// WIT package name for the reusable JSON stdlib component.
pub const STDLIB_PACKAGE: &str = "runtara:workflow-stdlib@0.1.0";

/// WIT package name for the runtime/SDK lifecycle component.
pub const RUNTIME_PACKAGE: &str = "runtara:workflow-runtime@0.4.0";

/// Runtime interface imported by compiled workflows and implemented by the host.
pub const RUNTIME_INTERFACE_NAME: &str = "runtara:workflow-runtime/runtime@0.4.0";

/// Previous contract, retained by the host for already-built workflow artifacts.
pub const LEGACY_RUNTIME_INTERFACE_NAME: &str = "runtara:workflow-runtime/runtime@0.3.0";

/// WIT package name for safe runtime connection resolution.
pub const CONNECTION_RESOLVER_PACKAGE: &str = "runtara:connection-resolver@0.2.0";

/// Fully-qualified component import name of the connection resolver.
pub const CONNECTION_RESOLVER_INTERFACE_NAME: &str = "runtara:connection-resolver/resolver@0.2.0";

/// Previous synchronous contract, retained for already-built workflow artifacts.
pub const LEGACY_CONNECTION_RESOLVER_INTERFACE_NAME: &str =
    "runtara:connection-resolver/resolver@0.1.0";

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

/// WIT text for `runtara:workflow-runtime@0.4.0`.
pub const RUNTIME_WIT: &str = include_str!("../wit/runtime/runtara-workflow-runtime.wit");

/// WIT text for `runtara:connection-resolver@0.2.0`.
pub const CONNECTION_RESOLVER_WIT: &str =
    include_str!("../wit/connection-resolver/runtara-connection-resolver.wit");

/// WIT text for `runtara:abi@0.1.0` (the neutral shared vocabulary).
pub const ABI_WIT: &str = include_str!("../wit/lifecycle/deps/abi/runtara-abi.wit");

/// WIT text for `runtara:workflow-lifecycle@0.2.0`.
pub const LIFECYCLE_WIT: &str = include_str!("../wit/lifecycle/runtara-workflow-lifecycle.wit");

/// WIT package of the compiler-emitted operation scope.
pub const OPERATION_PACKAGE: &str = "runtara:workflow-operation@0.1.0";

/// Component import name of the operation scope. Only compiled workflow logic
/// imports it; no agent may.
pub const OPERATION_SCOPE_INTERFACE_NAME: &str = "runtara:workflow-operation/scope@0.1.0";

/// WIT text for `runtara:workflow-operation@0.1.0`. It `use`s
/// `runtara:agent-suspension@0.1.0`, which must be in the resolve first.
pub const OPERATION_WIT: &str = include_str!("../wit/operation/runtara-workflow-operation.wit");

/// WIT package of the control service.
pub const CONTROL_PACKAGE: &str = "runtara:control@0.1.0";

/// Interface-name prefix of every control interface. Only the canonical
/// `control` agent may import one.
pub const CONTROL_INTERFACE_PREFIX: &str = "runtara:control/";

/// Control types, imported by the control agent.
pub const CONTROL_TYPES_INTERFACE_NAME: &str = "runtara:control/types@0.1.0";

/// Host control operations: real only in control executor stores.
pub const CONTROL_API_INTERFACE_NAME: &str = "runtara:control/api@0.1.0";

/// What the composed control copy forwards to.
pub const CONTROL_EXECUTOR_INTERFACE_NAME: &str = "runtara:control/executor@0.1.0";

/// Exported by the control agent, called only by the host executor.
pub const CONTROL_EXECUTION_INTERFACE_NAME: &str = "runtara:control/execution@0.1.0";

/// WIT text for `runtara:control@0.1.0`. It `use`s `runtara:agent@0.4.0` and
/// `runtara:agent-suspension@0.1.0`, which must be in the resolve first.
pub const CONTROL_WIT: &str = include_str!("../wit/control/runtara-control.wit");

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{CONNECTION_RESOLVER_PACKAGE, RUNTIME_PACKAGE, STDLIB_PACKAGE};
    use wit_parser::{Resolve, WorldItem};

    fn crate_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn database_wit_has_exactly_three_async_operations() {
        let mut resolve = Resolve::default();
        let id = resolve
            .push_str("database.wit", super::DATABASE_WIT)
            .unwrap();
        let package = &resolve.packages[id];
        let interface = &resolve.interfaces[package.interfaces["sql"]];
        assert_eq!(interface.functions.len(), 3);
        for name in ["query", "execute", "execute-batch"] {
            assert!(matches!(
                interface.functions[name].kind,
                wit_parser::FunctionKind::AsyncFreestanding
            ));
        }
        assert!(package.worlds.contains_key("database-client"));
        assert!(package.worlds.contains_key("database-host"));
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
            assert!(matches!(
                interface.functions[function].kind,
                wit_parser::FunctionKind::AsyncFreestanding
            ));
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
            "poll-signal",
            "is-cancelled",
            "check-signals",
            "poll-custom-signal",
            "register-input",
            "poll-input",
            "close-input",
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

#[cfg(test)]
mod execution_tests {
    #[test]
    fn execution_contract_uses_resources_and_async_joins() {
        use wit_parser::{FunctionKind, TypeDefKind};
        let mut resolve = wit_parser::Resolve::default();
        resolve.push_str("abi.wit", super::ABI_WIT).unwrap();
        resolve
            .push_str("lifecycle.wit", super::LIFECYCLE_WIT)
            .unwrap();
        let id = resolve
            .push_str("execution.wit", super::EXECUTION_WIT)
            .unwrap();
        let package = &resolve.packages[id];
        assert_eq!(package.name.to_string(), "runtara:workflow-execution@0.1.0");
        let tasks = &resolve.interfaces[package.interfaces["tasks"]];
        assert!(matches!(
            resolve.types[tasks.types["task"]].kind,
            TypeDefKind::Resource
        ));
        assert!(matches!(
            tasks.functions["start"].kind,
            FunctionKind::Freestanding
        ));
        for name in ["join", "release"] {
            assert!(matches!(
                tasks.functions[name].kind,
                FunctionKind::AsyncFreestanding
            ));
        }
        assert!(matches!(
            tasks.functions["request-cancel"].kind,
            FunctionKind::Freestanding
        ));
        for name in [
            "entry",
            "invocation-context",
            "execution-error",
            "cancel-status",
            "task-outcome",
        ] {
            assert!(tasks.types.contains_key(name));
        }
    }
}

#[cfg(test)]
mod outbound_http_tests {
    #[test]
    fn contract_has_async_request_and_explicit_destination() {
        use wit_parser::{FunctionKind, TypeDefKind};
        let mut resolve = wit_parser::Resolve::default();
        let id = resolve
            .push_str("outbound-http.wit", super::OUTBOUND_HTTP_WIT)
            .unwrap();
        let package = &resolve.packages[id];
        assert_eq!(package.name.to_string(), "runtara:outbound-http@0.1.0");
        let client = &resolve.interfaces[package.interfaces["client"]];
        assert!(matches!(
            client.functions["request"].kind,
            FunctionKind::AsyncFreestanding
        ));
        let TypeDefKind::Variant(destination) = &resolve.types[client.types["destination"]].kind
        else {
            panic!("destination must be a variant")
        };
        assert_eq!(
            destination
                .cases
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["connection", "public"]
        );
        let TypeDefKind::Record(options) = &resolve.types[client.types["request-options"]].kind
        else {
            panic!("request must be a record")
        };
        assert_eq!(
            options
                .fields
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            [
                "destination",
                "method",
                "headers",
                "body",
                "timeout-ms",
                "max-response-bytes"
            ]
        );
    }
}

#[cfg(test)]
mod control_tests {
    use wit_parser::{FunctionKind, Resolve, TypeDefKind};

    fn resolve() -> Resolve {
        let mut resolve = Resolve::default();
        resolve
            .push_str("agent.wit", runtara_agent_wit::RUNTARA_AGENT_WIT)
            .unwrap();
        resolve
            .push_str("agent-suspension.wit", runtara_agent_suspension::WIT)
            .unwrap();
        resolve
    }

    #[test]
    fn operation_scope_is_sync_and_takes_no_identity_from_agents() {
        let mut resolve = resolve();
        let id = resolve
            .push_str("operation.wit", super::OPERATION_WIT)
            .unwrap();
        let package = &resolve.packages[id];
        assert_eq!(package.name.to_string(), super::OPERATION_PACKAGE);
        let scope = &resolve.interfaces[package.interfaces["scope"]];
        assert_eq!(
            scope
                .functions
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["enter", "suspend", "exit", "release"]
        );
        assert!(
            scope
                .functions
                .values()
                .all(|function| matches!(function.kind, FunctionKind::Freestanding)),
            "the scope is compiler-called and synchronous"
        );
        assert_eq!(
            super::OPERATION_SCOPE_INTERFACE_NAME,
            format!(
                "runtara:workflow-operation/scope@{}",
                package.name.version.as_ref().unwrap()
            )
        );
    }

    #[test]
    fn only_host_called_execution_takes_a_continuation() {
        let mut resolve = resolve();
        let id = resolve.push_str("control.wit", super::CONTROL_WIT).unwrap();
        let package = &resolve.packages[id];
        assert_eq!(package.name.to_string(), super::CONTROL_PACKAGE);
        let params = |interface: &str| -> Vec<String> {
            resolve.interfaces[package.interfaces[interface]].functions["invoke"]
                .params
                .iter()
                .map(|param| param.name.clone())
                .collect()
        };
        assert_eq!(params("executor"), ["capability-id", "input"]);
        assert_eq!(
            params("execution"),
            ["capability-id", "input", "continuation"]
        );
        let api = &resolve.interfaces[package.interfaces["api"]];
        for function in api.functions.values() {
            assert!(matches!(function.kind, FunctionKind::AsyncFreestanding));
            assert!(
                function
                    .params
                    .iter()
                    .all(|param| !["tenant", "parent", "operation", "caller"]
                        .iter()
                        .any(|forbidden| param.name.contains(forbidden))),
                "identity never rides api arguments: {}",
                function.name
            );
        }
        for (world, name) in [
            ("control-client", super::CONTROL_EXECUTOR_INTERFACE_NAME),
            ("control-agent-host", super::CONTROL_API_INTERFACE_NAME),
        ] {
            let world = &resolve.worlds[package.worlds[world]];
            assert!(
                world
                    .imports
                    .keys()
                    .any(|key| resolve.name_world_key(key) == name)
            );
        }
        for name in [
            super::CONTROL_TYPES_INTERFACE_NAME,
            super::CONTROL_API_INTERFACE_NAME,
            super::CONTROL_EXECUTOR_INTERFACE_NAME,
            super::CONTROL_EXECUTION_INTERFACE_NAME,
        ] {
            assert!(name.starts_with(super::CONTROL_INTERFACE_PREFIX));
        }
    }

    fn enum_cases(resolve: &Resolve, types: &wit_parser::Interface, name: &str) -> Vec<String> {
        let TypeDefKind::Enum(cases) = &resolve.types[types.types[name]].kind else {
            panic!("{name} must be an enum");
        };
        cases.cases.iter().map(|case| case.name.clone()).collect()
    }

    fn variant_cases(resolve: &Resolve, types: &wit_parser::Interface, name: &str) -> Vec<String> {
        let TypeDefKind::Variant(cases) = &resolve.types[types.types[name]].kind else {
            panic!("{name} must be a variant");
        };
        cases.cases.iter().map(|case| case.name.clone()).collect()
    }

    fn record_fields(resolve: &Resolve, types: &wit_parser::Interface, name: &str) -> Vec<String> {
        let TypeDefKind::Record(record) = &resolve.types[types.types[name]].kind else {
            panic!("{name} must be a record");
        };
        record
            .fields
            .iter()
            .map(|field| field.name.clone())
            .collect()
    }

    /// 0.1.0 is frozen: wasmtime lifts exactly, so every function, case and
    /// field is pinned here, and a change needs a new package version.
    #[test]
    fn the_control_api_is_complete_and_frozen() {
        let mut resolve = resolve();
        let id = resolve.push_str("control.wit", super::CONTROL_WIT).unwrap();
        let package = &resolve.packages[id];
        let api = &resolve.interfaces[package.interfaces["api"]];
        let signatures: Vec<(String, Vec<String>)> = api
            .functions
            .values()
            .map(|function| {
                (
                    function.name.clone(),
                    function.params.iter().map(|p| p.name.clone()).collect(),
                )
            })
            .collect();
        let expected: Vec<(String, Vec<String>)> = [
            ("start", "request"),
            ("get", "instance-id"),
            ("query", "request"),
            ("list-pending-signals", "request"),
            ("send-signal", "request"),
            ("cancel", "request"),
            ("pause", "instance-id"),
            ("resume", "instance-id"),
            ("wait", "request"),
            ("poll-wait", "wait-id"),
        ]
        .iter()
        .map(|(name, param)| (name.to_string(), vec![param.to_string()]))
        .collect();
        assert_eq!(signatures, expected);
        assert!(
            api.functions
                .values()
                .all(|function| matches!(function.kind, FunctionKind::AsyncFreestanding))
        );

        let types = &resolve.interfaces[package.interfaces["types"]];
        let codes: Vec<String> = runtara_control_contract::ErrorCode::ALL
            .iter()
            .map(|code| code.wit_name().to_string())
            .collect();
        assert_eq!(codes.len(), 19);
        assert_eq!(
            enum_cases(&resolve, types, "error-code"),
            codes,
            "runtara-control-contract mirrors the WIT error codes in order"
        );
        assert_eq!(
            enum_cases(&resolve, types, "parent-close-policy"),
            runtara_control_contract::ParentClosePolicy::ALL
                .map(|policy| policy.wit_name().to_string())
        );
        for (name, cases) in [
            (
                "instance-status",
                &[
                    "queued",
                    "pending",
                    "running",
                    "suspended",
                    "completed",
                    "failed",
                    "cancelled",
                    "not-started",
                ][..],
            ),
            (
                "suspension-reason",
                &[
                    "paused",
                    "waiting-signal",
                    "waiting-instances",
                    "sleeping",
                    "shutdown",
                ],
            ),
            ("sort-field", &["created-at", "finished-at"]),
            ("sort-order", &["ascending", "descending"]),
            (
                "command-outcome",
                &["requested", "applied", "unchanged", "already-terminal"],
            ),
            ("wait-mode", &["all", "any"]),
            ("wait-resolution", &["satisfied", "deadline", "empty"]),
        ] {
            assert_eq!(enum_cases(&resolve, types, name), cases, "{name}");
        }
        for (name, cases) in [
            ("parent-filter", &["caller", "instance"][..]),
            ("signal-scope", &["instance", "workflow", "children"]),
            ("wait-poll", &["pending", "settled"]),
        ] {
            assert_eq!(variant_cases(&resolve, types, name), cases, "{name}");
        }
        for (name, fields) in [
            ("control-error", &["code", "message", "retry-after-ms"][..]),
            (
                "terminal-result",
                &[
                    "output",
                    "output-bytes",
                    "output-omitted",
                    "error",
                    "error-omitted",
                ],
            ),
            (
                "start-request",
                &[
                    "workflow-id",
                    "version",
                    "input",
                    "run-label",
                    "parent-close-policy",
                ],
            ),
            (
                "start-result",
                &[
                    "instance-id",
                    "workflow-id",
                    "version",
                    "run-label",
                    "replayed",
                ],
            ),
            ("wait-request", &["instance-ids", "mode", "deadline-ms"]),
            (
                "wait-progress",
                &["mode", "finished", "remaining", "deadline-ms"],
            ),
            (
                "target-outcome",
                &["instance-id", "status", "finished-at-ms", "terminal"],
            ),
            (
                "send-signal-request",
                &[
                    "instance-id",
                    "signal-id",
                    "action-key",
                    "request-id",
                    "payload",
                ],
            ),
            ("cancel-request", &["instance-id", "reason", "grace-ms"]),
            ("command-result", &["instance-id", "outcome", "replayed"]),
        ] {
            assert_eq!(record_fields(&resolve, types, name), fields, "{name}");
        }
    }

    /// The canonical layout of the suspension types equals the constants the
    /// emitter reads and the host mirrors (`runtara_agent_suspension::layout`).
    #[test]
    fn suspension_layout_constants_match_the_wit_size_align() {
        use runtara_agent_suspension::layout;
        use wit_parser::{Int, SizeAlign, Type};

        let mut resolve = resolve();
        let id = resolve.push_str("control.wit", super::CONTROL_WIT).unwrap();
        let mut sizes = SizeAlign::default();
        sizes.fill(&resolve);
        let bytes = |size: wit_parser::ArchitectureSize| size.size_wasm32() as u32;
        let alignment = |ty: &Type| match sizes.align(ty) {
            wit_parser::Alignment::Bytes(bytes) => bytes.get() as u32,
            wit_parser::Alignment::Pointer => 4,
        };
        let suspension_package = resolve
            .packages
            .iter()
            .find(|(_, package)| package.name.to_string() == runtara_agent_suspension::PACKAGE)
            .map(|(id, _)| id)
            .unwrap();
        let types = &resolve.interfaces[resolve.packages[suspension_package].interfaces["types"]];
        let ty = |name: &str| Type::Id(types.types[name]);
        let payload_offset = |name: &str| {
            let TypeDefKind::Variant(variant) = &resolve.types[types.types[name]].kind else {
                panic!("{name} must be a variant");
            };
            bytes(sizes.payload_offset(
                variant.tag(),
                variant.cases.iter().map(|case| case.ty.as_ref()),
            ))
        };

        assert_eq!(bytes(sizes.size(&ty("wake"))), layout::WAKE_SIZE);
        assert_eq!(alignment(&ty("wake")), layout::WAKE_ALIGN);
        assert_eq!(payload_offset("wake"), layout::WAKE_PAYLOAD_OFFSET);

        assert_eq!(
            bytes(sizes.size(&ty("suspension"))),
            layout::SUSPENSION_SIZE
        );
        assert_eq!(alignment(&ty("suspension")), layout::SUSPENSION_ALIGN);
        let TypeDefKind::Record(suspension) = &resolve.types[types.types["suspension"]].kind else {
            panic!("suspension must be a record");
        };
        let offsets: Vec<u32> = sizes
            .field_offsets(suspension.fields.iter().map(|field| &field.ty))
            .into_iter()
            .map(|(offset, _)| bytes(offset))
            .collect();
        assert_eq!(
            offsets,
            [
                layout::SUSPENSION_WAKES_OFFSET,
                layout::SUSPENSION_STATE_OFFSET
            ]
        );

        assert_eq!(bytes(sizes.size(&ty("outcome"))), layout::OUTCOME_SIZE);
        assert_eq!(alignment(&ty("outcome")), layout::OUTCOME_ALIGN);
        assert_eq!(payload_offset("outcome"), layout::OUTCOME_PAYLOAD_OFFSET);

        // `result<outcome, error-info>` as `execution.invoke` returns it.
        let package = &resolve.packages[id];
        let execution = &resolve.interfaces[package.interfaces["execution"]];
        let Some(Type::Id(result)) = execution.functions["invoke"].result else {
            panic!("execution.invoke returns a result");
        };
        let TypeDefKind::Result(result) = &resolve.types[result].kind else {
            panic!("execution.invoke returns a result");
        };
        assert_eq!(
            bytes(sizes.payload_offset(Int::U8, [result.ok.as_ref(), result.err.as_ref()])),
            layout::INVOKE_RESULT_PAYLOAD_OFFSET
        );
    }
}
