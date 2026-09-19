//! Bind privileged dependencies to the exact approved bundle bytes. Empty
//! instance imports survive serialization/precompilation and are checked by the
//! host loader before any guest code or credential lookup runs.
use super::{DirectCompileError, artifact_metadata::ResolvedComponentDependency};
use wasm_encoder::{
    ComponentImportSection, ComponentSection, ComponentTypeRef, ComponentTypeSection, Encode,
    InstanceType,
};

pub(super) fn pin_trusted_dependencies(
    wasm: &mut Vec<u8>,
    dependencies: &[ResolvedComponentDependency],
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
        pins.extend(artifact_pins(&component_bytes)?);
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

fn artifact_pins(wasm: &[u8]) -> Result<std::collections::BTreeSet<String>, DirectCompileError> {
    let mut pins = std::collections::BTreeSet::new();
    let mut depth = 0usize;
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        match payload.map_err(|e| DirectCompileError::Component(e.to_string()))? {
            wasmparser::Payload::Version { .. } => depth += 1,
            wasmparser::Payload::End(_) => depth -= 1,
            wasmparser::Payload::ComponentImportSection(imports) if depth == 1 => {
                for import in imports {
                    let import =
                        import.map_err(|e| DirectCompileError::Component(e.to_string()))?;
                    if import.name.0.starts_with("runtara:trusted-artifacts/") {
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
}
