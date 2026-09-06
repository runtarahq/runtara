//! Private native bundle carried inside the existing nonce/digest-bound protocol.
//! This is not a deployable workflow format and never accepts untrusted native code.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, ensure};
use runtara_workflow_wit::isolation_package::{Binding, InvocationManifest, PackageLimits, parse};
use serde::{Deserialize, Serialize};
use wasmtime::{Engine, component::Component};

use super::{MAX_PRECOMPILE_COMPONENT_BYTES, MAX_PRECOMPILED_COMPONENT_BYTES};

const MAGIC: &[u8; 8] = b"RTRNP001";
const MAGIC_V2: &[u8; 8] = b"RTRNP002";

/// Prepared native definitions; all mutable guest state is still per Store.
pub struct CompiledWorkflowPackage {
    pub root: Component,
    pub artifacts: BTreeMap<String, Component>,
    pub bindings: Vec<Binding>,
    pub invocations: Option<InvocationManifest>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    digest: String,
    offset: usize,
    length: usize,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    root_length: usize,
    members: Vec<Member>,
    bindings: Vec<Binding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    invocations: Option<InvocationManifest>,
}

pub(super) fn is_bundle(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC) || bytes.starts_with(MAGIC_V2)
}

pub(super) fn precompile(engine: &Engine, source: &[u8]) -> Result<Vec<u8>> {
    // The existing artifact size limit is the envelope. No smaller graph or
    // binding-count restriction is added by the new execution format.
    let limits = PackageLimits {
        total_bytes: MAX_PRECOMPILE_COMPONENT_BYTES,
        manifest_bytes: MAX_PRECOMPILE_COMPONENT_BYTES,
        artifacts: MAX_PRECOMPILE_COMPONENT_BYTES / 8,
        bindings: MAX_PRECOMPILE_COMPONENT_BYTES / 8,
    };
    let Some(package) = parse(source, limits)? else {
        return engine
            .precompile_component(source)
            .map_err(|error| anyhow::anyhow!("precompile workflow component: {error:#}"));
    };
    let root = engine.precompile_component(package.root)?;
    let root_length = root.len();
    ensure!(
        root_length <= MAX_PRECOMPILED_COMPONENT_BYTES,
        "precompiled root exceeds output limit"
    );
    let mut offset = root_length;
    let mut members = Vec::new();
    let mut bodies = vec![root];
    for (digest, source) in package.artifacts() {
        let body = engine.precompile_component(source)?;
        let end = offset
            .checked_add(body.len())
            .filter(|end| *end <= MAX_PRECOMPILED_COMPONENT_BYTES)
            .ok_or_else(|| anyhow::anyhow!("precompiled package exceeds output limit"))?;
        members.push(Member {
            digest: digest.clone(),
            offset,
            length: body.len(),
        });
        bodies.push(body);
        offset = end;
    }
    let index = serde_json::to_vec(&Index {
        root_length,
        members,
        bindings: package.bindings().values().cloned().collect(),
        invocations: package.invocations().cloned(),
    })?;
    ensure!(
        index.len() <= MAX_PRECOMPILE_COMPONENT_BYTES,
        "precompiled package index exceeds limit"
    );
    let total = 12usize
        .checked_add(index.len())
        .and_then(|n| n.checked_add(offset))
        .filter(|n| *n <= MAX_PRECOMPILED_COMPONENT_BYTES)
        .ok_or_else(|| anyhow::anyhow!("precompiled package frame exceeds output limit"))?;
    let mut encoded = Vec::with_capacity(total);
    encoded.extend_from_slice(if package.invocations().is_some() {
        MAGIC_V2
    } else {
        MAGIC
    });
    encoded.extend_from_slice(&u32::try_from(index.len())?.to_le_bytes());
    encoded.extend_from_slice(&index);
    for body in bodies {
        encoded.extend_from_slice(&body);
    }
    Ok(encoded)
}

struct View<'a> {
    root: &'a [u8],
    members: BTreeMap<String, &'a [u8]>,
    bindings: Vec<Binding>,
    invocations: Option<InvocationManifest>,
}

fn decode(bytes: &[u8]) -> Result<View<'_>> {
    ensure!(
        bytes.len() <= MAX_PRECOMPILED_COMPONENT_BYTES,
        "precompiled package exceeds limit"
    );
    ensure!(is_bundle(bytes), "invalid precompiled package header");
    let length = bytes
        .get(8..12)
        .ok_or_else(|| anyhow::anyhow!("truncated precompiled package index length"))?;
    let index_len = u32::from_le_bytes(length.try_into()?) as usize;
    ensure!(
        index_len <= MAX_PRECOMPILE_COMPONENT_BYTES,
        "precompiled package index exceeds limit"
    );
    let index_end = 12usize
        .checked_add(index_len)
        .filter(|n| *n <= bytes.len())
        .ok_or_else(|| anyhow::anyhow!("truncated precompiled package index"))?;
    let index: Index = serde_json::from_slice(&bytes[12..index_end])?;
    ensure!(
        bytes.starts_with(MAGIC_V2) == index.invocations.is_some(),
        "native invocation authority version mismatch"
    );
    let bodies = &bytes[index_end..];
    let root = bodies
        .get(..index.root_length)
        .filter(|b| !b.is_empty())
        .ok_or_else(|| anyhow::anyhow!("invalid precompiled root length"))?;
    let mut offset = index.root_length;
    let mut members = BTreeMap::new();
    let mut previous = None;
    for member in index.members {
        ensure!(
            member.offset == offset && member.length > 0,
            "invalid precompiled member layout"
        );
        ensure!(
            member.digest.len() == 64
                && member
                    .digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid source digest"
        );
        ensure!(
            previous
                .as_ref()
                .is_none_or(|digest| digest < &member.digest),
            "duplicate or unsorted precompiled member"
        );
        let end = offset
            .checked_add(member.length)
            .filter(|n| *n <= bodies.len())
            .ok_or_else(|| anyhow::anyhow!("truncated precompiled member"))?;
        previous = Some(member.digest.clone());
        members.insert(member.digest, &bodies[offset..end]);
        offset = end;
    }
    ensure!(offset == bodies.len(), "unindexed precompiled bytes");
    let mut names = BTreeSet::new();
    let mut used = BTreeSet::new();
    for binding in &index.bindings {
        ensure!(
            !binding.id.is_empty() && !binding.interface.is_empty() && names.insert(&binding.id),
            "invalid or duplicate binding"
        );
        ensure!(
            members.contains_key(&binding.artifact),
            "binding references missing precompiled member"
        );
        used.insert(&binding.artifact);
    }
    ensure!(used.len() == members.len(), "unused precompiled member");
    if let Some(invocations) = &index.invocations {
        invocations.validate(
            &index
                .bindings
                .iter()
                .map(|binding| (binding.id.clone(), binding.clone()))
                .collect(),
        )?;
    }
    Ok(View {
        root,
        members,
        bindings: index.bindings,
        invocations: index.invocations,
    })
}

/// Caller must establish trusted Wasmtime provenance before entering this function.
pub(super) unsafe fn deserialize(engine: &Engine, bytes: &[u8]) -> Result<CompiledWorkflowPackage> {
    if !is_bundle(bytes) {
        // SAFETY: the caller established the trusted worker/native cache boundary.
        return Ok(CompiledWorkflowPackage {
            root: unsafe { Component::deserialize(engine, bytes)? },
            artifacts: BTreeMap::new(),
            bindings: Vec::new(),
            invocations: None,
        });
    }
    let view = decode(bytes)?;
    // SAFETY: all members come from the same trusted, verified worker envelope.
    let root = unsafe { Component::deserialize(engine, view.root)? };
    let mut artifacts = BTreeMap::new();
    for (digest, body) in view.members {
        // SAFETY: same provenance as the root; decode already checked framing.
        artifacts.insert(digest, unsafe { Component::deserialize(engine, body)? });
    }
    Ok(CompiledWorkflowPackage {
        root,
        artifacts,
        bindings: view.bindings,
        invocations: view.invocations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EngineConfig, build_engine};
    use runtara_workflow_wit::isolation_package::{append, artifact_digest};

    fn root() -> Vec<u8> {
        wat::parse_str("(component)").unwrap()
    }

    #[test]
    fn invocation_authority_survives_native_transport_and_cannot_be_silently_dropped() {
        use runtara_workflow_wit::isolation_package::{
            AgentCallSite, InvocationCallSite, append_with_invocations,
        };
        let engine = build_engine(&EngineConfig {
            cache_dir: None,
            ..Default::default()
        })
        .unwrap();
        let child = root();
        for version in [1, 2, 3] {
            let expected = InvocationManifest {
                scope_paths: if version == 3 {
                    [(7, vec![Default::default()]), (8, vec![Default::default()])].into()
                } else {
                    Default::default()
                },
                call_sites: if version == 1 {
                    Vec::new()
                } else {
                    vec![
                        InvocationCallSite {
                            token: 7,
                            identity: 0,
                            agent_reference: 2,
                            caller_reference: 2,
                            domain: 0,
                        },
                        InvocationCallSite {
                            token: 8,
                            identity: 0,
                            agent_reference: 2,
                            caller_reference: 10,
                            domain: 3,
                        },
                    ]
                },
                version,
                workflow_id: "root".into(),
                agent_calls: vec![AgentCallSite {
                    binding: "agent:test".into(),
                    agent_id: "test".into(),
                    capability: "copy".into(),
                    step_id: "s".into(),
                    domains: vec![0, 3],
                }],
            };
            let package = append_with_invocations(
                &root(),
                &[&child],
                vec![Binding {
                    id: "agent:test".into(),
                    artifact: artifact_digest(&child),
                    interface: "test".into(),
                }],
                expected.clone(),
                PackageLimits {
                    total_bytes: 65536,
                    manifest_bytes: 32768,
                    artifacts: 1,
                    bindings: 1,
                },
            )
            .unwrap();
            let native = precompile(&engine, &package).unwrap();
            assert!(native.starts_with(MAGIC_V2));
            // SAFETY: our own engine just produced this entire native response.
            let loaded = unsafe { deserialize(&engine, &native) }.unwrap();
            assert_eq!(loaded.invocations, Some(expected));
            for mode in ["missing", "binding", "version"] {
                let end = 12 + u32::from_le_bytes(native[8..12].try_into().unwrap()) as usize;
                let mut index: Index = serde_json::from_slice(&native[12..end]).unwrap();
                match mode {
                    "missing" => index.invocations = None,
                    "binding" => {
                        index.invocations.as_mut().unwrap().agent_calls[0].binding =
                            "agent:other".into()
                    }
                    _ => index.invocations.as_mut().unwrap().version = 4,
                }
                let json = serde_json::to_vec(&index).unwrap();
                let mut changed = MAGIC_V2.to_vec();
                changed.extend_from_slice(&(json.len() as u32).to_le_bytes());
                changed.extend_from_slice(&json);
                changed.extend_from_slice(&native[end..]);
                assert!(decode(&changed).is_err(), "accepted {mode}");
            }
            let mut old_header = native;
            old_header[..8].copy_from_slice(MAGIC);
            assert!(decode(&old_header).is_err());
        }
    }

    #[test]
    fn legacy_native_bytes_remain_compatible() {
        let engine = build_engine(&EngineConfig {
            cache_dir: None,
            ..Default::default()
        })
        .unwrap();
        let bytes = precompile(&engine, &root()).unwrap();
        assert!(!is_bundle(&bytes));
        // SAFETY: bytes were just produced by this engine's precompiler.
        let loaded = unsafe { deserialize(&engine, &bytes) }.unwrap();
        assert!(loaded.artifacts.is_empty());
        assert!(loaded.bindings.is_empty());
        assert!(loaded.invocations.is_none());
    }

    #[test]
    fn all_children_are_precompiled_and_survive_trusted_roundtrip() {
        let engine = build_engine(&EngineConfig {
            cache_dir: None,
            ..Default::default()
        })
        .unwrap();
        let child = root();
        let bindings = (0..100)
            .map(|i| Binding {
                id: format!("call-{i}"),
                artifact: artifact_digest(&child),
                interface: "test".into(),
            })
            .collect();
        let package = append(
            &root(),
            &[&child],
            bindings,
            PackageLimits {
                total_bytes: 65536,
                manifest_bytes: 32768,
                artifacts: 1,
                bindings: 100,
            },
        )
        .unwrap();
        let native = precompile(&engine, &package).unwrap();
        assert!(is_bundle(&native));
        let view = decode(&native).unwrap();
        assert_eq!(view.members.len(), 1);
        // SAFETY: the complete native bundle was produced by this engine.
        let loaded = unsafe { deserialize(&engine, &native) }.unwrap();
        assert_eq!(loaded.artifacts.len(), 1);
        assert_eq!(loaded.bindings.len(), 100);
        for end in 0..native.len() {
            assert!(decode(&native[..end]).is_err(), "prefix {end}");
        }
        let mut extra = native;
        extra.push(1);
        assert!(decode(&extra).is_err());
    }

    #[test]
    fn package_integrity_is_checked_before_native_compilation() {
        let engine = build_engine(&EngineConfig {
            cache_dir: None,
            ..Default::default()
        })
        .unwrap();
        let child = root();
        let mut package = append(
            &root(),
            &[&child],
            vec![Binding {
                id: "child".into(),
                artifact: artifact_digest(&child),
                interface: "test".into(),
            }],
            PackageLimits {
                total_bytes: 65536,
                manifest_bytes: 32768,
                artifacts: 1,
                bindings: 1,
            },
        )
        .unwrap();
        *package.last_mut().unwrap() ^= 1;
        assert!(precompile(&engine, &package).is_err());
    }
}
