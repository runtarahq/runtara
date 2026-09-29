//! Bind privileged dependencies to the exact approved bundle bytes. Empty
//! instance imports survive serialization/precompilation; the host checks each
//! trusted call against them before any credential lookup runs.
use super::{DirectCompileError, artifact_metadata::ResolvedComponentDependency};
use wasm_encoder::{
    ComponentImportSection, ComponentSection, ComponentTypeRef, ComponentTypeSection, Encode,
    InstanceType,
};

pub(super) fn pin_trusted_dependencies(
    wasm: &mut Vec<u8>,
    dependencies: &[ResolvedComponentDependency],
    components_dir: &std::path::Path,
) -> Result<(), DirectCompileError> {
    let mut pins = std::collections::BTreeSet::new();
    for dep in dependencies {
        let Some(agent_id) = &dep.metadata.agent_id else {
            continue;
        };
        let Some(meta) = &dep.metadata.meta else {
            continue;
        };
        let bytes = std::fs::read(dep.wasm_path.with_extension("meta.json"))?;
        // Recheck the sidecar identity captured during resolution. Metadata
        // changed mid-build must never be paired with an older artifact hash.
        if super::sha256_hex(&bytes) != meta.file.sha256 {
            return Err(DirectCompileError::Component(
                "agent metadata changed during composition".into(),
            ));
        }
        // Scoped workflow-agent children are packaged separately, so WAC
        // cannot lift their transitive imports into the root. Preserve those
        // version requirements explicitly before appending the child package.
        let component_bytes = std::fs::read(&dep.wasm_path)?;
        // A staged workflow-agent that calls control carries the control pin
        // it was composed with; the import allowlist refuses a pin anywhere
        // else. Either kind must name the version in the bundle.
        for pin in artifact_pins(&component_bytes)? {
            require_bundled_version(components_dir, agent_id, &dep.wasm_path, &pin)?;
            pins.insert(pin);
        }
        // The host runs control only for the exact control bytes a workflow
        // composed, from the primary components dir (decision D2).
        if runtara_dsl::agent_meta::canonical_agent_id(agent_id)
            == runtara_dsl::agent_meta::CONTROL_AGENT_ID
        {
            let artifact = dep.metadata.wasm.as_ref().ok_or_else(|| {
                DirectCompileError::Component("missing control artifact identity".into())
            })?;
            if dep.wasm_path.parent() != Some(components_dir)
                || super::sha256_hex(&component_bytes) != artifact.sha256
            {
                return Err(DirectCompileError::Component(
                    "the control agent must be the one in the components dir, unchanged during \
                     composition"
                        .into(),
                ));
            }
            pins.insert(runtara_dsl::agent_meta::builtin_artifact_import(
                agent_id,
                &artifact.sha256,
                &meta.file.sha256,
            ));
        }
        let info: serde_json::Value = serde_json::from_slice(&bytes)?;
        if info
            .get("capabilities")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|caps| {
                caps.iter().any(|cap| {
                    cap.get("trusted").and_then(serde_json::Value::as_bool) == Some(true)
                })
            })
        {
            let artifact = dep.metadata.wasm.as_ref().ok_or_else(|| {
                DirectCompileError::Component("missing trusted artifact identity".into())
            })?;
            if super::sha256_hex(&component_bytes) != artifact.sha256 {
                return Err(DirectCompileError::Component(
                    "trusted component changed during composition".into(),
                ));
            }
            pins.insert(runtara_dsl::agent_meta::trusted_artifact_import(
                agent_id,
                &artifact.sha256,
                &meta.file.sha256,
            ));
        }
    }
    let existing = artifact_pins(wasm)?;
    pins.retain(|pin| !existing.contains(pin));
    if pins.is_empty() {
        return Ok(());
    }
    let types = wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::all())
        .validate_all(wasm)
        .map_err(|e| DirectCompileError::Component(e.to_string()))?;
    let index = types.as_ref().component_type_count();
    append_pins(wasm, index, pins.iter().map(String::as_str));
    Ok(())
}

/// A published workflow-agent keeps the pins of the trusted and control
/// built-in versions it was composed against; nothing restages it when the
/// operator upgrades one. A parent composed from it would pin a version no
/// host runs, so every launch would compile, look unready and compile again.
/// Refuse it with an actionable diagnostic instead: the version `pin` names
/// must be the one in the primary component bundle. The error is typed so the
/// server records the stale pin with the failure and retries it after a
/// republish.
fn require_bundled_version(
    components_dir: &std::path::Path,
    dependency: &str,
    wasm_path: &std::path::Path,
    pin: &str,
) -> Result<(), DirectCompileError> {
    let (agent, bundled) = pinned_and_bundled(components_dir, pin).ok_or_else(|| {
        DirectCompileError::Component(format!(
            "agent `{dependency}` carries a malformed built-in artifact pin"
        ))
    })?;
    if bundled.as_deref() == Some(pin) {
        return Ok(());
    }
    Err(DirectCompileError::StaleTrustedDependency {
        dependency: dependency.to_owned(),
        agent: agent.to_owned(),
        pins: vec![pin.to_owned()],
        wasm_path: wasm_path.to_owned(),
    })
}

/// Whether the workflow-agent artifact at `wasm_path` still pins a trusted
/// built-in version that is not the one in `components_dir`.
///
/// A compile refused with [`DirectCompileError::StaleTrustedDependency`] read
/// that artifact before the failure is recorded; a republish can replace it in
/// between. `false` (republished, removed or unreadable) means the refusal
/// describes an input that no longer exists, so the server retries instead of
/// recording a failure that only the next republish would release.
pub fn staged_dependency_is_stale(
    components_dir: &std::path::Path,
    wasm_path: &std::path::Path,
) -> bool {
    let Ok(bytes) = std::fs::read(wasm_path) else {
        return false;
    };
    let Ok(pins) = artifact_pins(&bytes) else {
        return false;
    };
    pins.iter().any(|pin| {
        pinned_and_bundled(components_dir, pin)
            .is_some_and(|(_, bundled)| bundled.as_deref() != Some(pin.as_str()))
    })
}

/// The built-in agent `pin` names, and the pin of that agent's version in
/// `components_dir` (`None` when the bundle does not ship it). `None` for a
/// string that is no trusted or control pin.
fn pinned_and_bundled<'a>(
    components_dir: &std::path::Path,
    pin: &'a str,
) -> Option<(&'a str, Option<String>)> {
    if let Some(agent) = runtara_dsl::agent_meta::trusted_artifact_import_agent_id(pin) {
        return Some((agent, bundled_trusted_pin(components_dir, agent)));
    }
    let (agent, _) = runtara_dsl::agent_meta::parse_builtin_artifact_import(pin)?;
    Some((agent, bundled_builtin_pin(components_dir, agent)))
}

/// The `runtara:trusted-artifacts/*` pin of the version of trusted built-in
/// `agent` in `components_dir`, or `None` when the bundle does not ship it.
/// This is the pin a workflow compiled against that bundle records.
pub fn bundled_trusted_pin(components_dir: &std::path::Path, agent: &str) -> Option<String> {
    let component = crate::direct_wasm::component::agent_component(agent);
    let wasm = std::fs::read(components_dir.join(&component.bundle_wasm_filename)).ok()?;
    let meta = std::fs::read(components_dir.join(&component.bundle_meta_filename)).ok()?;
    Some(runtara_dsl::agent_meta::trusted_artifact_import(
        agent,
        &super::sha256_hex(&wasm),
        &super::sha256_hex(&meta),
    ))
}

/// The `runtara:builtin-artifacts/*` pin of host-executed built-in `agent`
/// (the control agent) in `components_dir`, or `None` when the bundle does
/// not ship it. This is the pin a workflow compiled against that bundle
/// records, and the one a server approves at boot.
pub fn bundled_builtin_pin(components_dir: &std::path::Path, agent: &str) -> Option<String> {
    let component = crate::direct_wasm::component::agent_component(agent);
    let wasm = std::fs::read(components_dir.join(&component.bundle_wasm_filename)).ok()?;
    let meta = std::fs::read(components_dir.join(&component.bundle_meta_filename)).ok()?;
    Some(runtara_dsl::agent_meta::builtin_artifact_import(
        agent,
        &super::sha256_hex(&wasm),
        &super::sha256_hex(&meta),
    ))
}

/// The approved built-in versions an artifact pins: its top-level
/// `runtara:trusted-artifacts/*` and `runtara:builtin-artifacts/*` imports. An isolated package's catalog is a
/// trailing custom section, so the root's pins are read the same way.
pub fn trusted_artifact_pins(
    wasm: &[u8],
) -> Result<std::collections::BTreeSet<String>, DirectCompileError> {
    artifact_pins(wasm)
}

fn artifact_pins(wasm: &[u8]) -> Result<std::collections::BTreeSet<String>, DirectCompileError> {
    let mut pins = std::collections::BTreeSet::new();
    let mut depth = 0usize;
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        match payload.map_err(|e| DirectCompileError::Component(e.to_string()))? {
            wasmparser::Payload::Version { encoding, .. } => {
                // A core module has no component imports; reading it as an
                // artifact with no pins would claim it is always ready.
                if depth == 0 && encoding != wasmparser::Encoding::Component {
                    return Err(DirectCompileError::Component(
                        "artifact is not a component".into(),
                    ));
                }
                depth += 1
            }
            wasmparser::Payload::End(_) => depth -= 1,
            wasmparser::Payload::ComponentImportSection(imports) if depth == 1 => {
                for import in imports {
                    let import =
                        import.map_err(|e| DirectCompileError::Component(e.to_string()))?;
                    if import.name.0.starts_with("runtara:trusted-artifacts/")
                        || import
                            .name
                            .0
                            .starts_with(runtara_dsl::agent_meta::BUILTIN_ARTIFACTS_PREFIX)
                    {
                        pins.insert(import.name.0.to_owned());
                    }
                }
            }
            _ => {}
        }
    }
    Ok(pins)
}

fn append_pins<'a>(wasm: &mut Vec<u8>, index: u32, pins: impl Iterator<Item = &'a str>) {
    let mut types = ComponentTypeSection::new();
    types.instance(&InstanceType::new());
    wasm.push(types.id());
    types.encode(wasm);
    let mut imports = ComponentImportSection::new();
    for pin in pins {
        imports.import(pin, ComponentTypeRef::Instance(index));
    }
    wasm.push(imports.id());
    imports.encode(wasm);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn content_pin_is_a_real_component_import() {
        let mut wasm = wasm_encoder::Component::new().finish();
        let pin = runtara_dsl::agent_meta::trusted_artifact_import(
            "s3-storage",
            &"a".repeat(64),
            &"b".repeat(64),
        );
        append_pins(&mut wasm, 0, std::iter::once(pin.as_str()));
        wasmparser::Validator::new_with_features(wasmparser::WasmFeatures::all())
            .validate_all(&wasm)
            .unwrap();
        let mut found = false;
        for payload in wasmparser::Parser::new(0).parse_all(&wasm) {
            if let wasmparser::Payload::ComponentImportSection(imports) = payload.unwrap() {
                found = imports.into_iter().any(|item| item.unwrap().name.0 == pin);
            }
        }
        assert!(found);
    }

    #[test]
    fn a_staged_pin_must_name_the_bundled_trusted_version() {
        let bundle = tempfile::tempdir().unwrap();
        let wasm = b"s3 component bytes".to_vec();
        let meta = b"{\"id\":\"s3-storage\"}".to_vec();
        std::fs::write(bundle.path().join("runtara_agent_s3_storage.wasm"), &wasm).unwrap();
        let meta_path = bundle.path().join("runtara_agent_s3_storage.meta.json");
        std::fs::write(&meta_path, &meta).unwrap();
        let bundled = runtara_dsl::agent_meta::trusted_artifact_import(
            "s3-storage",
            &super::super::sha256_hex(&wasm),
            &super::super::sha256_hex(&meta),
        );
        let staged = bundle.path().join("runtara_agent_wrapper.wasm");
        require_bundled_version(bundle.path(), "wrapper", &staged, &bundled).unwrap();

        assert_eq!(
            bundled_trusted_pin(bundle.path(), "s3-storage").as_deref(),
            Some(bundled.as_str())
        );

        // An upgraded bundle, or one without the built-in at all, refuses the
        // older pin with a typed error naming the workflow-agent to republish
        // and carrying the stale pin, so the server can record it.
        std::fs::write(&meta_path, [meta.as_slice(), b"\n"].concat()).unwrap();
        assert_ne!(
            bundled_trusted_pin(bundle.path(), "s3-storage").as_deref(),
            Some(bundled.as_str())
        );
        let upgraded =
            require_bundled_version(bundle.path(), "wrapper", &staged, &bundled).unwrap_err();
        let DirectCompileError::StaleTrustedDependency {
            dependency,
            agent,
            pins,
            wasm_path,
        } = &upgraded
        else {
            panic!("{upgraded:?}");
        };
        assert_eq!(
            (dependency.as_str(), agent.as_str(), pins.as_slice()),
            ("wrapper", "s3-storage", std::slice::from_ref(&bundled))
        );
        assert_eq!(wasm_path, &staged, "the server re-reads this artifact");
        let upgraded = upgraded.to_string();
        assert!(upgraded.contains("workflow-agent `wrapper`"), "{upgraded}");
        assert!(upgraded.contains("`s3-storage`"), "{upgraded}");
        assert!(upgraded.contains("republish"), "{upgraded}");
        std::fs::remove_file(&meta_path).unwrap();
        assert_eq!(bundled_trusted_pin(bundle.path(), "s3-storage"), None);
        let removed = require_bundled_version(bundle.path(), "wrapper", &staged, &bundled)
            .unwrap_err()
            .to_string();
        assert!(removed.contains("republish"), "{removed}");
        assert!(matches!(
            require_bundled_version(bundle.path(), "wrapper", &staged, "runtara:trusted/x@0.1.0"),
            Err(DirectCompileError::Component(_))
        ));
    }

    #[test]
    fn unreadable_artifacts_never_record_an_empty_pin_set() {
        let mut pinned = wasm_encoder::Component::new().finish();
        let pin = runtara_dsl::agent_meta::trusted_artifact_import(
            "s3-storage",
            &"a".repeat(64),
            &"b".repeat(64),
        );
        append_pins(&mut pinned, 0, std::iter::once(pin.as_str()));
        let core_module = wasm_encoder::Module::new().finish();
        for bytes in [
            &[][..],
            b"not a wasm artifact",
            &pinned[..pinned.len() - 3],
            &core_module,
        ] {
            assert!(trusted_artifact_pins(bytes).is_err(), "{bytes:?}");
        }
        assert_eq!(
            trusted_artifact_pins(&pinned).unwrap(),
            std::collections::BTreeSet::from([pin])
        );
    }

    #[test]
    fn recorded_pins_are_the_root_imports_even_when_packaged() {
        let pin = |agent: &str| {
            runtara_dsl::agent_meta::trusted_artifact_import(
                agent,
                &"a".repeat(64),
                &"b".repeat(64),
            )
        };
        let mut child = wasm_encoder::Component::new().finish();
        append_pins(&mut child, 0, std::iter::once(pin("child-only").as_str()));
        let mut root = wasm_encoder::Component::new().finish();
        assert!(trusted_artifact_pins(&root).unwrap().is_empty());
        append_pins(&mut root, 0, std::iter::once(pin("s3-storage").as_str()));
        let limits = runtara_invocation_contract::PackageLimits {
            total_bytes: 1024 * 1024,
            manifest_bytes: 64 * 1024,
            artifacts: 4,
            bindings: 4,
        };
        let digest = runtara_invocation_contract::artifact_digest(&child);
        let packaged = runtara_invocation_contract::append(
            &root,
            &[child.as_slice()],
            vec![runtara_invocation_contract::Binding {
                id: "child".into(),
                artifact: digest,
                interface: "runtara:agent-child/capabilities@0.1.0".into(),
            }],
            limits,
        )
        .unwrap();
        assert_eq!(
            trusted_artifact_pins(&packaged).unwrap(),
            std::collections::BTreeSet::from([pin("s3-storage")])
        );
    }

    #[test]
    fn a_republished_dependency_is_no_longer_stale() {
        let bundle = tempfile::tempdir().unwrap();
        let (wasm, meta) = (b"s3 component bytes".to_vec(), b"{}".to_vec());
        std::fs::write(bundle.path().join("runtara_agent_s3_storage.wasm"), &wasm).unwrap();
        std::fs::write(
            bundle.path().join("runtara_agent_s3_storage.meta.json"),
            &meta,
        )
        .unwrap();
        let bundled = bundled_trusted_pin(bundle.path(), "s3-storage").unwrap();
        let old = runtara_dsl::agent_meta::trusted_artifact_import(
            "s3-storage",
            &super::super::sha256_hex(&wasm),
            &"f".repeat(64),
        );
        let staged = bundle.path().join("runtara_agent_wrapper.wasm");
        let stage = |pins: &[&str]| {
            let mut child = wasm_encoder::Component::new().finish();
            if !pins.is_empty() {
                append_pins(&mut child, 0, pins.iter().copied());
            }
            std::fs::write(&staged, child).unwrap();
        };

        // Built against the older version: still stale, alone or beside a
        // current pin.
        stage(&[old.as_str()]);
        assert!(staged_dependency_is_stale(bundle.path(), &staged));
        stage(&[bundled.as_str(), old.as_str()]);
        assert!(staged_dependency_is_stale(bundle.path(), &staged));
        // Republished against the installed bundle, or no longer trusted.
        stage(&[bundled.as_str()]);
        assert!(!staged_dependency_is_stale(bundle.path(), &staged));
        stage(&[]);
        assert!(!staged_dependency_is_stale(bundle.path(), &staged));
        // Removed or unreadable: the next compile reports what is really there.
        std::fs::write(&staged, b"not a component").unwrap();
        assert!(!staged_dependency_is_stale(bundle.path(), &staged));
        std::fs::remove_file(&staged).unwrap();
        assert!(!staged_dependency_is_stale(bundle.path(), &staged));
    }
}
