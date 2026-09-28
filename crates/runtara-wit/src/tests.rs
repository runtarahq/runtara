// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Structural checks of the runtara WIT packages.

use wit_parser::{FunctionKind, Package, Resolve, TypeDefKind, WorldItem};

use crate::{AgentShape, PACKAGES, agent_package};

fn resolve() -> Resolve {
    crate::resolve().expect("every runtara package resolves")
}

fn package<'a>(resolve: &'a Resolve, name: &str) -> &'a Package {
    resolve
        .packages
        .iter()
        .map(|(_, package)| package)
        .find(|package| package.name.to_string() == name)
        .unwrap_or_else(|| panic!("package {name} is in the resolve"))
}

fn interface<'a>(resolve: &'a Resolve, package: &Package, name: &str) -> &'a wit_parser::Interface {
    &resolve.interfaces[package.interfaces[name]]
}

/// Every name constant names an interface that exists, at the shared version.
#[test]
fn every_name_constant_names_a_real_interface() {
    let resolve = resolve();
    let mut names = std::collections::BTreeSet::new();
    for (id, _) in resolve.interfaces.iter() {
        if let Some(name) = resolve.id_of(id) {
            names.insert(name);
        }
    }
    for name in [
        crate::agent::TYPES,
        crate::agent::SUSPENSION,
        crate::agent::CONTINUATION,
        crate::agent::CAPABILITIES,
        crate::host::HTTP,
        crate::host::SQL,
        crate::host::CONNECTIONS,
        crate::host::TIMERS,
        crate::workflow::LIFECYCLE,
        crate::workflow::RUNTIME,
        crate::workflow::TASKS,
        crate::workflow::OPERATION,
        crate::workflow::WAITS,
        crate::stdlib::JSON,
        crate::trusted::EXECUTOR,
        crate::trusted::EXECUTION,
        crate::control::TYPES,
        crate::control::API,
        crate::control::EXECUTOR,
        crate::control::EXECUTION,
    ] {
        assert!(names.contains(name), "{name} is not a runtara interface");
        assert!(name.ends_with(&format!("@{}", crate::VERSION)));
    }
    for (prefix, name) in [
        (crate::agent::TYPES_PREFIX, crate::agent::TYPES),
        (crate::host::PREFIX, crate::host::HTTP),
        (crate::workflow::PREFIX, crate::workflow::RUNTIME),
        (
            crate::workflow::OPERATION_PREFIX,
            crate::workflow::OPERATION,
        ),
        (crate::workflow::WAITS_PREFIX, crate::workflow::WAITS),
        (crate::control::PREFIX, crate::control::API),
    ] {
        assert!(
            name.starts_with(prefix),
            "{name} does not start with {prefix}"
        );
    }
    let packages: Vec<String> = resolve
        .packages
        .iter()
        .map(|(_, package)| package.name.to_string())
        .filter(|name| name.starts_with("runtara:"))
        .collect();
    assert_eq!(packages.len(), PACKAGES.len());
    assert!(names.contains(crate::wasi::MONOTONIC_CLOCK));
    for name in [
        crate::agent::PACKAGE,
        crate::host::PACKAGE,
        crate::workflow::PACKAGE,
        crate::stdlib::PACKAGE,
        crate::trusted::PACKAGE,
        crate::control::PACKAGE,
    ] {
        assert!(packages.iter().any(|package| package == name), "{name}");
    }
}

/// The generated per-agent package resolves for every shape, and declares
/// exactly the interfaces and world items the shape asks for.
#[test]
fn agent_package_resolves_for_every_shape() {
    for scoped in [false, true] {
        for suspendable in [false, true] {
            for trusted in [false, true] {
                for control in [false, true] {
                    let shape = AgentShape {
                        scoped,
                        suspendable,
                        trusted,
                        control,
                    };
                    let mut resolve = resolve();
                    let wit = agent_package("probe-agent", shape);
                    let id = resolve
                        .push_str("probe.wit", &wit)
                        .unwrap_or_else(|error| panic!("{shape:?}: {error:#}\n{wit}"));
                    let package = &resolve.packages[id];
                    assert_eq!(
                        package.name.to_string(),
                        format!("runtara:agent-probe-agent@{}", crate::VERSION)
                    );
                    let capabilities = crate::capabilities_interface(shape);
                    assert!(package.interfaces.contains_key(capabilities));
                    assert_eq!(package.interfaces.contains_key("suspendable"), suspendable);
                    let world = &resolve.worlds[package.worlds["agent"]];
                    let imports: Vec<String> = world
                        .imports
                        .keys()
                        .map(|key| resolve.name_world_key(key))
                        .collect();
                    let exports: Vec<String> = world
                        .exports
                        .keys()
                        .map(|key| resolve.name_world_key(key))
                        .collect();
                    let has = |names: &[String], name: &str| names.iter().any(|n| n == name);
                    assert_eq!(has(&imports, crate::agent::CONTINUATION), suspendable);
                    assert_eq!(has(&imports, crate::control::API), control);
                    assert_eq!(has(&imports, crate::control::EXECUTOR), control);
                    assert_eq!(has(&exports, crate::trusted::EXECUTION), trusted);
                    assert_eq!(has(&exports, crate::control::EXECUTION), control);
                }
            }
        }
    }
}

#[test]
fn host_services_are_async_and_explicit() {
    let resolve = resolve();
    let host = package(&resolve, crate::host::PACKAGE);
    let sql = interface(&resolve, host, "sql");
    assert_eq!(sql.functions.len(), 3);
    for name in ["query", "execute", "execute-batch"] {
        assert!(matches!(
            sql.functions[name].kind,
            FunctionKind::AsyncFreestanding
        ));
    }
    let connections = interface(&resolve, host, "connections");
    for name in ["describe", "resolve-resource"] {
        assert!(matches!(
            connections.functions[name].kind,
            FunctionKind::AsyncFreestanding
        ));
    }
    let timers = interface(&resolve, host, "timers");
    for name in ["sleep", "abort-after"] {
        assert!(matches!(
            timers.functions[name].kind,
            FunctionKind::AsyncFreestanding
        ));
    }
    let http = interface(&resolve, host, "http");
    assert!(matches!(
        http.functions["request"].kind,
        FunctionKind::AsyncFreestanding
    ));
    let TypeDefKind::Variant(destination) = &resolve.types[http.types["destination"]].kind else {
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
    assert_eq!(
        record_fields(&resolve, http, "request-options"),
        [
            "destination",
            "method",
            "headers",
            "body",
            "timeout-ms",
            "max-response-bytes"
        ]
    );
    for (world, interface) in [
        ("http-client", crate::host::HTTP),
        ("sql-client", crate::host::SQL),
        ("connections-client", crate::host::CONNECTIONS),
        ("timers-client", crate::host::TIMERS),
    ] {
        let world = &resolve.worlds[host.worlds[world]];
        assert!(world.exports.is_empty());
        assert!(
            world
                .imports
                .keys()
                .any(|key| resolve.name_world_key(key) == interface)
        );
    }
}

#[test]
fn stdlib_exports_the_json_world() {
    let resolve = resolve();
    let stdlib = package(&resolve, crate::stdlib::PACKAGE);
    let interface_id = stdlib.interfaces["json"];
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
    let world = &resolve.worlds[stdlib.worlds["workflow-stdlib"]];
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
fn workflow_package_declares_the_workflow_abi() {
    let resolve = resolve();
    let workflow = package(&resolve, crate::workflow::PACKAGE);

    let lifecycle = interface(&resolve, workflow, "lifecycle");
    assert!(lifecycle.functions.contains_key("invoke"));
    for type_name in ["signal-wait", "wake", "outcome"] {
        assert!(
            lifecycle.types.contains_key(type_name),
            "missing lifecycle type {type_name}"
        );
    }

    let runtime = interface(&resolve, workflow, "runtime");
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
            runtime.functions.contains_key(function),
            "missing runtime function {function}"
        );
    }
    for type_name in ["signal-info", "custom-signal-info", "checkpoint-result"] {
        assert!(
            runtime.types.contains_key(type_name),
            "missing runtime type {type_name}"
        );
    }

    let tasks = interface(&resolve, workflow, "tasks");
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

    let operation = interface(&resolve, workflow, "operation");
    assert_eq!(
        operation
            .functions
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["enter", "suspend", "exit", "release"]
    );
    assert!(
        operation
            .functions
            .values()
            .all(|function| matches!(function.kind, FunctionKind::Freestanding)),
        "the operation scope is compiler-called and synchronous"
    );

    let waits = interface(&resolve, workflow, "waits");
    assert_eq!(
        waits
            .functions
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["register", "poll", "release"]
    );
    assert!(
        waits
            .functions
            .values()
            .all(|function| matches!(function.kind, FunctionKind::Freestanding)),
        "the waits are compiler-called and synchronous"
    );
    let params = |name: &str| {
        waits.functions[name]
            .params
            .iter()
            .map(|param| param.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(params("register"), ["key", "request"]);
    assert_eq!(params("poll"), ["key"]);
    assert_eq!(params("release"), ["key"]);
    assert!(waits.functions["release"].result.is_none());
}

#[test]
fn control_forwarding_is_a_plain_call_and_takes_no_identity() {
    let resolve = resolve();
    let package = package(&resolve, crate::control::PACKAGE);
    let params = |interface: &str| -> Vec<String> {
        resolve.interfaces[package.interfaces[interface]].functions["invoke"]
            .params
            .iter()
            .map(|param| param.name.clone())
            .collect()
    };
    assert_eq!(params("executor"), ["capability-id", "input"]);
    assert_eq!(params("execution"), ["capability-id", "input"]);
    for interface in ["executor", "execution"] {
        let invoke = &resolve.interfaces[package.interfaces[interface]].functions["invoke"];
        let Some(wit_parser::Type::Id(result)) = invoke.result else {
            panic!("{interface}.invoke returns a result");
        };
        let TypeDefKind::Result(result) = &resolve.types[result].kind else {
            panic!("{interface}.invoke returns a result");
        };
        assert!(
            matches!(result.ok, Some(wit_parser::Type::Id(list))
                if matches!(resolve.types[list].kind,
                    TypeDefKind::List(wit_parser::Type::U8))),
            "{interface}.invoke answers the capability's JSON output"
        );
    }
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
        ("control-client", crate::control::EXECUTOR),
        ("control-agent-host", crate::control::API),
    ] {
        let world = &resolve.worlds[package.worlds[world]];
        assert!(
            world
                .imports
                .keys()
                .any(|key| resolve.name_world_key(key) == name)
        );
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

/// 1.0.0 is frozen: wasmtime lifts exactly, so every function, case and
/// field is pinned here, and a change needs a new package version.
#[test]
fn the_control_api_is_complete_and_frozen() {
    let resolve = resolve();
    let package = package(&resolve, crate::control::PACKAGE);
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
    assert_eq!(codes.len(), 18);
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
    ] {
        assert_eq!(enum_cases(&resolve, types, name), cases, "{name}");
    }
    for (name, cases) in [
        ("parent-filter", &["caller", "instance"][..]),
        ("signal-scope", &["instance", "workflow", "children"]),
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
