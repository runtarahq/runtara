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
    let original = package();
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
