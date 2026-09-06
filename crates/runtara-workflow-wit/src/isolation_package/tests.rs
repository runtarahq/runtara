use super::*;

fn limits() -> PackageLimits {
    PackageLimits {
        total_bytes: 65536,
        manifest_bytes: 8192,
        artifacts: 16,
        bindings: 128,
    }
}

fn component(name: &str) -> Vec<u8> {
    let mut bytes = COMPONENT_HEADER.to_vec();
    bytes.push(0);
    write_u32((name.len() + 1) as u32, &mut bytes);
    write_u32(name.len() as u32, &mut bytes);
    bytes.extend_from_slice(name.as_bytes());
    bytes
}

fn binding(id: &str, bytes: &[u8]) -> Binding {
    Binding {
        id: id.into(),
        artifact: artifact_digest(bytes),
        interface: "test:agent/capabilities@1.0.0".into(),
    }
}

fn package() -> Vec<u8> {
    let a = component("one");
    let b = component("two");
    append(
        COMPONENT_HEADER,
        &[&a, &b],
        vec![binding("a", &a), binding("b", &b)],
        limits(),
    )
    .unwrap()
}

fn rewritten(mutate: impl FnOnce(&mut Manifest)) -> Vec<u8> {
    rewritten_from(package(), mutate)
}

fn rewritten_from(original: Vec<u8>, mutate: impl FnOnce(&mut Manifest)) -> Vec<u8> {
    let (_, section) = catalog_section(&original).unwrap().unwrap();
    let len = u32::from_le_bytes(section[..4].try_into().unwrap()) as usize;
    let mut manifest: Manifest = serde_json::from_slice(&section[4..4 + len]).unwrap();
    mutate(&mut manifest);
    let json = serde_json::to_vec(&manifest).unwrap();
    let mut payload = Vec::new();
    write_u32(SECTION_NAME.len() as u32, &mut payload);
    payload.extend_from_slice(SECTION_NAME.as_bytes());
    payload.extend_from_slice(&(json.len() as u32).to_le_bytes());
    payload.extend_from_slice(&json);
    payload.extend_from_slice(&section[4 + len..]);
    let mut out = COMPONENT_HEADER.to_vec();
    out.push(0);
    write_u32(payload.len() as u32, &mut out);
    out.extend_from_slice(&payload);
    out
}

fn invocations() -> InvocationManifest {
    InvocationManifest {
        scope_paths: Default::default(),
        call_sites: Vec::new(),
        version: 1,
        workflow_id: "workflow::雪".into(),
        agent_calls: vec![AgentCallSite {
            binding: "agent:utils".into(),
            agent_id: "utils".into(),
            capability: "random-double".into(),
            step_id: "step::雪".into(),
            domains: vec![0, 3],
        }],
    }
}
fn package_v2() -> Vec<u8> {
    let child = component("utils");
    append_with_invocations(
        COMPONENT_HEADER,
        &[&child],
        vec![binding("agent:utils", &child)],
        invocations(),
        limits(),
    )
    .unwrap()
}

#[test]
fn invocation_manifest_roundtrips_with_digested_code_and_legacy_encoding_is_unchanged() {
    let bytes = package_v2();
    let parsed = parse(&bytes, limits()).unwrap().unwrap();
    assert_eq!(parsed.invocations(), Some(&invocations()));
    assert_eq!(parsed.artifacts().len(), 1);
    assert_eq!(bytes, package_v2());
    let legacy = package();
    let legacy_parsed = parse(&legacy, limits()).unwrap().unwrap();
    assert!(legacy_parsed.invocations().is_none());
    let (_, section) = catalog_section(&legacy).unwrap().unwrap();
    let len = u32::from_le_bytes(section[..4].try_into().unwrap()) as usize;
    let json: serde_json::Value = serde_json::from_slice(&section[4..4 + len]).unwrap();
    assert_eq!(json["version"], 1);
    assert!(json.get("invocations").is_none());
}

#[test]
fn invocation_authority_versions_references_domains_and_duplicate_identities_are_checked() {
    for mode in [
        "version",
        "absent",
        "old-envelope",
        "binding",
        "agent",
        "duplicate",
        "overlap",
        "domain",
        "domain-order",
        "empty",
    ] {
        let bytes = rewritten_from(package_v2(), |m| match mode {
            "absent" => m.invocations = None,
            "old-envelope" => m.version = 1,
            _ => {
                let inv = m.invocations.as_mut().unwrap();
                match mode {
                    "version" => inv.version = 4,
                    "binding" => inv.agent_calls[0].binding = "agent:missing".into(),
                    "agent" => inv.agent_calls[0].agent_id = "missing".into(),
                    "duplicate" => inv.agent_calls.push(inv.agent_calls[0].clone()),
                    "overlap" => {
                        let mut site = inv.agent_calls[0].clone();
                        site.domains = vec![4];
                        inv.agent_calls.push(site);
                    }
                    "domain" => inv.agent_calls[0].domains = vec![6],
                    "domain-order" => inv.agent_calls[0].domains = vec![3, 0],
                    "empty" => inv.agent_calls.clear(),
                    _ => unreachable!(),
                }
            }
        });
        assert!(parse(&bytes, limits()).is_err(), "accepted {mode}");
    }
    assert!(
        parse(
            &rewritten_from(package_v2(), |m| m
                .invocations
                .as_mut()
                .unwrap()
                .agent_calls[0]
                .step_id = "x".repeat(10000)),
            limits()
        )
        .is_err()
    );
}

#[test]
fn repeated_bindings_share_one_artifact_and_encoding_is_deterministic() {
    let a = component("one");
    let b = component("two");
    let bindings = vec![binding("z", &a), binding("a", &a), binding("b", &b)];
    let first = append(COMPONENT_HEADER, &[&a, &b, &a], bindings.clone(), limits()).unwrap();
    let second = append(
        COMPONENT_HEADER,
        &[&b, &a],
        bindings.into_iter().rev().collect(),
        limits(),
    )
    .unwrap();
    assert_eq!(first, second);
    let parsed = parse(&first, limits()).unwrap().unwrap();
    assert_eq!(parsed.artifacts().len(), 2);
    assert_eq!(parsed.bindings().len(), 3);
    assert_eq!(parsed.root, COMPONENT_HEADER);
    assert_eq!(parsed.resolve("z").unwrap().1, a);
    assert_eq!(
        parsed.resolve("a").unwrap().1.as_ptr(),
        parsed.resolve("z").unwrap().1.as_ptr()
    );
    assert!(parsed.resolve("missing").is_none());
}

#[test]
fn legacy_component_remains_unmodified_and_has_no_catalog() {
    let bytes = component("legacy");
    assert!(parse(&bytes, limits()).unwrap().is_none());
}

#[test]
fn all_truncated_prefixes_and_corrupt_lengths_are_rejected_without_panics() {
    let bytes = package();
    for end in 0..bytes.len() {
        let result = parse(&bytes[..end], limits());
        if end == 8 {
            assert!(result.unwrap().is_none());
        } else {
            assert!(result.is_err(), "prefix {end}");
        }
    }
    let mut malformed = COMPONENT_HEADER.to_vec();
    malformed.extend_from_slice(&[0, 255, 255, 255, 255, 127]);
    assert!(matches!(
        parse(&malformed, limits()),
        Err(PackageError::InvalidFraming)
    ));
}

#[test]
fn changed_child_bytes_fail_digest_verification() {
    let mut bytes = package();
    *bytes.last_mut().unwrap() ^= 1;
    assert!(matches!(
        parse(&bytes, limits()),
        Err(PackageError::DigestMismatch)
    ));
}

#[test]
fn overlapping_duplicate_and_unused_artifacts_are_rejected() {
    for bytes in [
        rewritten(|m| m.artifacts[1].offset = 0),
        rewritten(|m| m.artifacts[1].digest = m.artifacts[0].digest.clone()),
    ] {
        assert!(matches!(
            parse(&bytes, limits()),
            Err(PackageError::InvalidLayout)
        ));
    }
    let bytes = rewritten(|m| {
        m.bindings.pop();
    });
    assert!(matches!(
        parse(&bytes, limits()),
        Err(PackageError::MissingArtifact)
    ));
}

#[test]
fn invalid_binding_and_version_are_rejected() {
    let duplicate = rewritten(|m| m.bindings.push(m.bindings[0].clone()));
    assert!(matches!(
        parse(&duplicate, limits()),
        Err(PackageError::DuplicateBinding)
    ));
    let missing = rewritten(|m| m.bindings[0].artifact = "not-a-digest".into());
    assert!(matches!(
        parse(&missing, limits()),
        Err(PackageError::MissingArtifact)
    ));
    let version = rewritten(|m| m.version = 2);
    assert!(matches!(
        parse(&version, limits()),
        Err(PackageError::UnsupportedVersion)
    ));
}

#[test]
fn size_manifest_and_count_limits_apply_to_decoding_and_encoding() {
    let bytes = package();
    for bounds in [
        PackageLimits {
            total_bytes: bytes.len() - 1,
            ..limits()
        },
        PackageLimits {
            manifest_bytes: 4,
            ..limits()
        },
        PackageLimits {
            artifacts: 1,
            ..limits()
        },
        PackageLimits {
            bindings: 1,
            ..limits()
        },
    ] {
        assert!(matches!(
            parse(&bytes, bounds),
            Err(PackageError::LimitExceeded)
        ));
        let a = component("one");
        let b = component("two");
        assert_eq!(
            append(
                COMPONENT_HEADER,
                &[&a, &b],
                vec![binding("a", &a), binding("b", &b)],
                bounds
            ),
            Err(PackageError::LimitExceeded)
        );
    }
}

#[test]
fn catalog_must_be_last_and_cannot_be_nested_in_an_artifact() {
    let bytes = package();
    assert_eq!(
        append(&bytes, &[], vec![], limits()),
        Err(PackageError::AlreadyPackaged)
    );
    assert_eq!(
        append(
            COMPONENT_HEADER,
            &[&bytes],
            vec![binding("child", &bytes)],
            limits()
        ),
        Err(PackageError::AlreadyPackaged)
    );
    let mut suffix = bytes.clone();
    suffix.extend_from_slice(&[0, 1, 0]);
    assert!(matches!(
        parse(&suffix, limits()),
        Err(PackageError::InvalidLayout)
    ));
}

#[test]
fn nested_component_custom_sections_are_not_root_catalogs() {
    let nested = package();
    let mut outer = COMPONENT_HEADER.to_vec();
    outer.push(4);
    write_u32(nested.len() as u32, &mut outer);
    outer.extend_from_slice(&nested);
    assert!(parse(&outer, limits()).unwrap().is_none());
}

#[test]
fn native_or_core_module_headers_cannot_be_packaged_as_components() {
    for bytes in [b"\x7fELF....".as_slice(), b"\0asm\x01\0\0\0".as_slice()] {
        assert_eq!(
            append(
                COMPONENT_HEADER,
                &[bytes],
                vec![binding("a", bytes)],
                limits()
            ),
            Err(PackageError::NotComponent)
        );
    }
}

fn qualified_invocations() -> InvocationManifest {
    let mut inventory = invocations();
    inventory.version = 2;
    inventory.call_sites = vec![
        InvocationCallSite {
            token: 7,
            identity: 0,
            agent_reference: 2,
            caller_reference: 2,
            domain: 0,
        },
        InvocationCallSite {
            token: 9,
            identity: 0,
            agent_reference: 2,
            caller_reference: 10,
            domain: 3,
        },
        InvocationCallSite {
            token: 13,
            identity: 0,
            agent_reference: 2,
            caller_reference: 11,
            domain: 3,
        },
        InvocationCallSite {
            token: 19,
            identity: 0,
            agent_reference: 20,
            caller_reference: 20,
            domain: 0,
        },
    ];
    inventory
}

#[test]
fn qualified_inventory_roundtrips_distinct_definitions_and_callers_with_token_gaps() {
    let child = component("utils");
    let inventory = qualified_invocations();
    let bytes = append_with_invocations(
        COMPONENT_HEADER,
        &[&child],
        vec![binding("agent:utils", &child)],
        inventory.clone(),
        limits(),
    )
    .unwrap();
    assert_eq!(
        parse(&bytes, limits()).unwrap().unwrap().invocations(),
        Some(&inventory)
    );
    let old_json = serde_json::to_value(invocations()).unwrap();
    assert!(old_json.get("call_sites").is_none());
    assert_eq!(
        serde_json::from_value::<InvocationManifest>(old_json).unwrap(),
        invocations()
    );
}

#[test]
fn qualified_inventory_rejects_ambiguous_or_uncovered_authority_at_admission() {
    for mode in [
        "old-version",
        "duplicate-token",
        "token-order",
        "missing-identity",
        "wrong-domain",
        "duplicate-origin",
        "foreign-self",
        "uncovered",
        "conflicting-definition",
    ] {
        let bytes = rewritten_from(package_v2(), |m| {
            let mut inv = qualified_invocations();
            match mode {
                "old-version" => inv.version = 1,
                "duplicate-token" => inv.call_sites[1].token = 7,
                "token-order" => inv.call_sites.swap(0, 1),
                "missing-identity" => inv.call_sites[0].identity = u32::MAX,
                "wrong-domain" => inv.call_sites[0].domain = 5,
                "duplicate-origin" => inv.call_sites[2].caller_reference = 10,
                "foreign-self" => inv.call_sites[0].caller_reference = 3,
                "uncovered" => inv.call_sites.retain(|site| site.domain == 0),
                "conflicting-definition" => {
                    let mut other = inv.agent_calls[0].clone();
                    other.step_id = "z".into();
                    other.domains = vec![3];
                    inv.agent_calls.push(other);
                    inv.call_sites[2].identity = 1;
                }
                _ => unreachable!(),
            }
            m.invocations = Some(inv);
        });
        assert!(parse(&bytes, limits()).is_err(), "accepted {mode}");
    }
}

fn scoped_invocations() -> InvocationManifest {
    let mut inventory = qualified_invocations();
    inventory.version = 3;
    inventory.scope_paths = inventory
        .call_sites
        .iter()
        .map(|site| {
            (
                site.token,
                vec![InvocationScopePattern {
                    namespace: vec![ChildScopePattern {
                        step_id: "child::雪".into(),
                        loops: vec![LoopPattern(LoopKind::Split, "outer".into())],
                    }],
                    loops: vec![LoopPattern(LoopKind::While, "inner".into())],
                }],
            )
        })
        .collect();
    inventory
}

#[test]
fn scoped_inventory_roundtrips_and_rejects_missing_duplicate_or_unversioned_scopes() {
    let valid = rewritten_from(package_v2(), |m| m.invocations = Some(scoped_invocations()));
    assert_eq!(
        parse(&valid, limits()).unwrap().unwrap().invocations(),
        Some(&scoped_invocations())
    );
    for mode in ["old", "missing", "extra", "duplicate", "order"] {
        let bytes = rewritten_from(valid.clone(), |m| {
            let inv = m.invocations.as_mut().unwrap();
            match mode {
                "old" => inv.version = 2,
                "missing" => {
                    inv.scope_paths.remove(&7);
                }
                "extra" => {
                    inv.scope_paths.insert(u32::MAX, vec![]);
                }
                "duplicate" => {
                    let pattern = inv.scope_paths[&7][0].clone();
                    inv.scope_paths.get_mut(&7).unwrap().push(pattern);
                }
                "order" => {
                    inv.scope_paths
                        .get_mut(&7)
                        .unwrap()
                        .push(InvocationScopePattern::default());
                }
                _ => unreachable!(),
            }
        });
        assert!(parse(&bytes, limits()).is_err(), "accepted {mode}");
    }
    // Unreachable definitions have no permitted paths, rather than authority
    // over arbitrary ancestry. They can still be transported without execution.
    let empty = rewritten_from(valid, |m| {
        m.invocations
            .as_mut()
            .unwrap()
            .scope_paths
            .insert(7, vec![]);
    });
    assert!(parse(&empty, limits()).is_ok());
}

#[test]
fn scope_resolution_checks_each_loop_and_child_frame_and_exact_inherited_namespace() {
    use serde_json::json;
    let inv = scoped_invocations();
    let key = json!([
        "agent",
        "workflow::雪",
        [[
            "child",
            "workflow::雪",
            [["Split", "outer", 4294967295u32]],
            ["child::雪"]
        ]],
        [["While", "inner", 23]],
        ["utils", "random-double", "step::雪"]
    ]);
    let encode = |key: &serde_json::Value| format!("runtara:v3:{key}:aaaaaaah:aaaaaaaa");
    let resolve = |key: &serde_json::Value, inherited: &[NamespaceFrame]| {
        inv.resolve_scoped_agent_invocation(
            "agent:utils",
            "random-double",
            &encode(key),
            2,
            inherited,
        )
    };
    assert!(resolve(&key, &[]).is_ok());
    for mode in [
        "no-child",
        "extra-child",
        "child-id",
        "child-workflow",
        "parent-loop-id",
        "parent-loop-kind",
        "no-parent-loop",
        "loop-id",
        "loop-kind",
        "extra-loop",
        "no-loop",
    ] {
        let mut forged = key.clone();
        match mode {
            "no-child" => forged[2] = json!([]),
            "extra-child" => {
                let frame = forged[2][0].clone();
                forged[2].as_array_mut().unwrap().push(frame);
            }
            "child-id" => forged[2][0][3][0] = "sibling".into(),
            "child-workflow" => forged[2][0][1] = "foreign".into(),
            "parent-loop-id" => forged[2][0][2][0][1] = "sibling".into(),
            "parent-loop-kind" => forged[2][0][2][0][0] = "While".into(),
            "no-parent-loop" => forged[2][0][2] = json!([]),
            "loop-id" => forged[3][0][1] = "sibling".into(),
            "loop-kind" => forged[3][0][0] = "Split".into(),
            "extra-loop" => forged[3]
                .as_array_mut()
                .unwrap()
                .push(json!(["While", "inner", 0])),
            "no-loop" => forged[3] = json!([]),
            _ => unreachable!(),
        }
        // The flat identity remains valid: namespace membership is the reason
        // this request must not reach the scope factory.
        assert!(
            inv.resolve_agent_invocation("agent:utils", "random-double", &encode(&forged), 2)
                .is_ok()
        );
        assert!(resolve(&forged, &[]).is_err(), "accepted {mode}");
    }
    let inherited = NamespaceFrame::ToolChild {
        workflow_id: "parent".into(),
        loops: vec![],
        ai_step_id: "ai".into(),
        label: "tool".into(),
        call_counter: 8,
    };
    let mut nested = key.clone();
    nested[2]
        .as_array_mut()
        .unwrap()
        .insert(0, json!(["tool-child", "parent", [], ["ai", "tool", 8]]));
    assert!(resolve(&nested, std::slice::from_ref(&inherited)).is_ok());
    assert!(resolve(&nested, &[]).is_err());
    nested[2][0][3][2] = 9.into();
    assert!(resolve(&nested, &[inherited]).is_err());
    assert!(
        qualified_invocations()
            .resolve_scoped_agent_invocation("agent:utils", "random-double", &encode(&key), 2, &[])
            .is_err()
    );
}
