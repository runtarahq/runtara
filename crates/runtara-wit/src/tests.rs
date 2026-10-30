// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Structural checks of the runtara WIT packages.

use wit_parser::{Package, Resolve, TypeDefKind, WorldItem};

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
        crate::agent::CAPABILITIES,
        crate::stdlib::JSON,
        crate::trusted::EXECUTION,
    ] {
        assert!(names.contains(name), "{name} is not a runtara interface");
        assert!(name.ends_with(&format!("@{}", crate::VERSION)));
    }
    assert!(crate::agent::TYPES.starts_with(crate::agent::TYPES_PREFIX));
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
        crate::stdlib::PACKAGE,
        crate::trusted::PACKAGE,
    ] {
        assert!(packages.iter().any(|package| package == name), "{name}");
    }
}

/// The generated per-agent package resolves for every shape, and declares
/// exactly the interfaces and exports the shape asks for, importing no host
/// interface.
#[test]
fn agent_package_resolves_for_every_shape() {
    for scoped in [false, true] {
        for trusted in [false, true] {
            let shape = AgentShape { scoped, trusted };
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
            let world = &resolve.worlds[package.worlds["agent"]];
            // Agents reach the host over the workflow API: they import only
            // the shared types their `invoke` uses.
            let imports: Vec<String> = world
                .imports
                .keys()
                .map(|key| resolve.name_world_key(key))
                .collect();
            assert!(
                imports.iter().all(|name| name == crate::agent::TYPES),
                "{shape:?}: {imports:?}"
            );
            let exports: Vec<String> = world
                .exports
                .keys()
                .map(|key| resolve.name_world_key(key))
                .collect();
            assert_eq!(
                exports.iter().any(|name| name == crate::trusted::EXECUTION),
                trusted
            );
            assert_eq!(exports.len(), 1 + usize::from(trusted), "{exports:?}");
        }
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
        "while-cache-key",
        "while-result",
        "while-output-from-result",
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
    // Only the types `json` uses: the stdlib calls no host function.
    let imports: Vec<_> = world
        .imports
        .keys()
        .map(|key| resolve.name_world_key(key))
        .collect();
    assert_eq!(imports, [crate::agent::TYPES]);
    assert_eq!(world.exports.len(), 1);
    assert!(
        world
            .exports
            .values()
            .any(|item| matches!(item, WorldItem::Interface { id, .. } if *id == interface_id))
    );
}

/// The host serves no workflow interface: child runs are `runtara:api`'s.
#[test]
fn no_workflow_package_remains() {
    let resolve = resolve();
    assert!(
        resolve
            .packages
            .iter()
            .all(|(_, package)| package.name.to_string() != "runtara:workflow@1.0.0")
    );
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

#[test]
fn agents_and_workflows_share_one_outcome() {
    let resolve = resolve();
    let agent = package(&resolve, crate::agent::PACKAGE);
    let types = interface(&resolve, agent, "types");
    assert_eq!(
        variant_cases(&resolve, types, "wake"),
        ["at", "on-signal", "on-resume", "instances"]
    );
    assert_eq!(
        variant_cases(&resolve, types, "outcome"),
        ["completed", "suspended"]
    );
    assert_eq!(
        record_fields(&resolve, types, "suspension"),
        ["wakes", "state"]
    );
    assert_eq!(
        record_fields(&resolve, types, "signal-wait"),
        ["checkpoint-id", "deadline-ms"]
    );
    let mut resolve = resolve;
    let entry = crate::agent_package(crate::workflow::ENTRY_AGENT_ID, AgentShape::default());
    let id = resolve.push_str("entry.wit", &entry).unwrap();
    let world = &resolve.worlds[resolve.packages[id].worlds["agent"]];
    assert!(
        world
            .exports
            .keys()
            .any(|key| resolve.name_world_key(key) == crate::workflow::ENTRY)
    );
}

/// `error-info.details` was appended, so every earlier field keeps its offset
/// and the emitter's hand-laid writers stay valid: the record is 80 bytes and
/// `details` (tag, ptr, len) sits at 68/72/76 within it.
#[test]
fn error_info_appends_details_without_moving_a_field() {
    let resolve = resolve();
    let agent = package(&resolve, crate::agent::PACKAGE);
    let types = interface(&resolve, agent, "types");
    let id = types.types["error-info"];
    let TypeDefKind::Record(record) = &resolve.types[id].kind else {
        panic!("error-info is a record");
    };
    let names: Vec<_> = record.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "code",
            "message",
            "category",
            "severity",
            "retryable",
            "retry-after-ms",
            "attributes",
            "details"
        ]
    );
    let mut sizes = wit_parser::SizeAlign::default();
    sizes.fill(&resolve);
    let ty = wit_parser::Type::Id(id);
    assert_eq!(sizes.size(&ty).size_wasm32(), 80);
    let offsets: Vec<_> = sizes
        .field_offsets(record.fields.iter().map(|f| &f.ty))
        .into_iter()
        .map(|(offset, _)| offset.size_wasm32())
        .collect();
    assert_eq!(offsets, [0, 8, 16, 24, 32, 40, 56, 68]);
}
